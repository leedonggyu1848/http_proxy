mod redact;
mod socket;

use std::collections::{HashMap, VecDeque};
use std::fs::{self, File};
use std::io::{self, Read as _};
use std::mem::MaybeUninit;
use std::net::SocketAddr;
use std::os::fd::AsRawFd as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, OnceLock, PoisonError};
use std::time::{Duration, SystemTime};

use http_inspector_exchange::{ProcessAncestor, ProcessIdentity, ProcessInfo, ProcessUnavailable};
use rustix::fs::{Mode, OFlags, RawDir, openat, readlinkat_raw};

use super::ProcessStatus;

const RECENT_LIMIT: usize = 16;
const NEW_PID_WINDOW: u32 = 24;
const PARENT_LIMIT: usize = 8;
const COMMAND_BYTES: u64 = 16 * 1024;

static BROWSERS: LazyLock<Mutex<HashMap<ProcessIdentity, SocketAddr>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Default)]
pub(crate) struct Lookup {
    recent: Mutex<VecDeque<u32>>,
    accounts: OnceLock<Accounts>,
}

#[derive(Default)]
struct Accounts {
    usernames: HashMap<u32, String>,
    boot: Option<SystemTime>,
}

impl Accounts {
    fn load() -> Self {
        let passwd = fs::read_to_string("/etc/passwd").unwrap_or_default();
        let usernames = passwd
            .lines()
            .filter_map(|line| {
                let mut fields = line.split(':');
                Some((
                    fields.nth(2)?.parse().ok()?,
                    line.split(':').next()?.to_owned(),
                ))
            })
            .collect();
        let boot = fs::read_to_string("/proc/stat").ok().and_then(|stat| {
            let seconds = stat
                .lines()
                .find_map(|line| line.strip_prefix("btime "))?
                .parse()
                .ok()?;
            SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(seconds))
        });
        Self { usernames, boot }
    }

    fn start_time(&self, ticks: u64) -> Option<SystemTime> {
        let hz = rustix::param::clock_ticks_per_second().max(1);
        let seconds = ticks / hz;
        let nanos = u32::try_from((ticks % hz).saturating_mul(1_000_000_000) / hz).ok()?;
        self.boot?.checked_add(Duration::new(seconds, nanos))
    }
}

impl Lookup {
    pub(crate) fn find(
        &self,
        peer: SocketAddr,
        proxy: SocketAddr,
    ) -> Result<ProcessInfo, ProcessUnavailable> {
        let owner = socket::find(peer, proxy)?;
        if owner.uid != rustix::process::geteuid().as_raw() {
            return Err(ProcessUnavailable::DifferentUser(owner.uid));
        }
        let link = format!("socket:[{}]", owner.inode);
        let found = self.find_owner(owner.uid, &link)?;
        self.remember(found.stat.identity.pid);
        Ok(found.snapshot(owner.uid, self.accounts.get_or_init(Accounts::load), proxy))
    }

    fn find_owner(&self, uid: u32, link: &str) -> Result<ProcOwner, ProcessUnavailable> {
        let recent = self
            .recent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        for &pid in &recent {
            if let Some(found) = find_pid(pid, uid, link) {
                return Ok(found);
            }
        }
        if let Some(found) = newly_created_owner(uid, link, &recent) {
            return Ok(found);
        }
        let entries = fs::read_dir("/proc").map_err(|error| match error.kind() {
            io::ErrorKind::PermissionDenied => ProcessUnavailable::PermissionDenied,
            _ => ProcessUnavailable::OwnerUnavailable,
        })?;
        let mut candidates: Vec<u32> = entries
            .flatten()
            .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
            .collect();
        // New connections from short-lived commands usually have recent PIDs.
        // Try those before walking the descriptors of long-running processes.
        candidates.sort_unstable_by(|left, right| right.cmp(left));
        candidates
            .into_iter()
            .filter(|pid| !recent.contains(pid))
            .find_map(|pid| find_pid(pid, uid, link))
            .ok_or(ProcessUnavailable::OwnerUnavailable)
    }

    fn remember(&self, pid: u32) {
        let mut recent = self.recent.lock().unwrap_or_else(PoisonError::into_inner);
        recent.retain(|stored| *stored != pid);
        recent.push_front(pid);
        recent.truncate(RECENT_LIMIT);
    }
}

fn newly_created_owner(uid: u32, link: &str, recent: &VecDeque<u32>) -> Option<ProcOwner> {
    let last: u32 = fs::read_to_string("/proc/sys/kernel/ns_last_pid")
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let first = last.saturating_sub(NEW_PID_WINDOW).max(1);
    // The kernel also allocates thread IDs from this sequence. The process
    // leader owns the identity shown in the inspector.
    (first..=last)
        .rev()
        .filter(|pid| !recent.contains(pid))
        .find_map(|pid| {
            let owner = find_pid(pid, uid, link)?;
            let status = fs::read_to_string(owner.path.join("status")).ok()?;
            let group: u32 = status
                .lines()
                .find_map(|line| line.strip_prefix("Tgid:"))?
                .trim()
                .parse()
                .ok()?;
            if group == pid {
                Some(owner)
            } else {
                find_pid(group, uid, link)
            }
        })
}

fn find_pid(pid: u32, uid: u32, link: &str) -> Option<ProcOwner> {
    let directory = File::open(pid_path(pid)).ok()?;
    if directory.metadata().ok()?.uid() != uid {
        return None;
    }
    // Keep the proc directory open while reading through it, so Linux cannot
    // redirect a later read to a new process that reused this PID.
    if !has_socket(&directory, link) {
        return None;
    }
    let path = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    let stat = ProcStat::read_at(&path, pid).ok()?;
    Some(ProcOwner {
        directory,
        path,
        stat,
    })
}

struct ProcOwner {
    directory: File,
    path: PathBuf,
    stat: ProcStat,
}

impl ProcOwner {
    fn snapshot(self, uid: u32, accounts: &Accounts, proxy: SocketAddr) -> ProcessInfo {
        let info = self.stat.snapshot(&self.path, uid, accounts, proxy);
        drop(self.directory);
        info
    }
}

fn has_socket(process: &File, link: &str) -> bool {
    let Ok(descriptors) = openat(
        process,
        c"fd",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    ) else {
        return false;
    };
    let mut directory_buffer = [MaybeUninit::uninit(); 2048];
    let mut entries = RawDir::new(&descriptors, &mut directory_buffer);
    // A socket link has "socket:[", at most 20 digits for u64, and "]".
    let mut link_buffer = [0; 32];
    while let Some(Ok(entry)) = entries.next() {
        if entry
            .file_name()
            .to_bytes()
            .first()
            .is_some_and(u8::is_ascii_digit)
            && let Ok(length) =
                readlinkat_raw(&descriptors, entry.file_name(), &mut link_buffer[..])
            && link_buffer.get(..length) == Some(link.as_bytes())
        {
            return true;
        }
    }
    false
}

fn pid_path(pid: u32) -> PathBuf {
    Path::new("/proc").join(pid.to_string())
}

pub(super) fn status(identity: ProcessIdentity) -> ProcessStatus {
    match ProcStat::read(identity.pid) {
        Ok(stat) if stat.identity != identity || stat.exited => ProcessStatus::Exited,
        Ok(_) => ProcessStatus::Running,
        Err(error) if error.kind() == io::ErrorKind::NotFound => ProcessStatus::Exited,
        Err(_) => ProcessStatus::Unknown,
    }
}

pub(crate) struct BrowserRegistration(ProcessIdentity);

pub(crate) fn register_browser(pid: Option<u32>, proxy: SocketAddr) -> Option<BrowserRegistration> {
    let identity = ProcStat::read(pid?).ok()?.identity;
    BROWSERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(identity, proxy);
    Some(BrowserRegistration(identity))
}

impl Drop for BrowserRegistration {
    fn drop(&mut self) {
        BROWSERS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.0);
    }
}

struct ProcStat {
    identity: ProcessIdentity,
    name: String,
    parent: u32,
    exited: bool,
}

impl ProcStat {
    fn read(pid: u32) -> io::Result<Self> {
        Self::read_at(&pid_path(pid), pid)
    }

    fn read_at(path: &Path, pid: u32) -> io::Result<Self> {
        let bytes = fs::read(path.join("stat"))?;
        Self::parse(&String::from_utf8_lossy(&bytes), pid)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid process stat"))
    }

    fn parse(line: &str, pid: u32) -> Option<Self> {
        let (_, name_and_fields) = line.split_once(" (")?;
        let (name, fields) = name_and_fields.rsplit_once(") ")?;
        let mut fields = fields.split_ascii_whitespace();
        let exited = matches!(fields.next()?, "Z" | "X" | "x");
        let parent = fields.next()?.parse().ok()?;
        let start_ticks = fields.nth(17)?.parse().ok()?;
        Some(Self {
            identity: ProcessIdentity { pid, start_ticks },
            name: name.to_owned(),
            parent,
            exited,
        })
    }

    fn snapshot(
        self,
        path: &Path,
        uid: u32,
        accounts: &Accounts,
        proxy: SocketAddr,
    ) -> ProcessInfo {
        let executable = path.join("exe").read_link().ok();
        let mut bytes = Vec::new();
        let redacted_command = File::open(path.join("cmdline")).ok().and_then(|file| {
            let length = file.take(COMMAND_BYTES + 1).read_to_end(&mut bytes).ok()?;
            (u64::try_from(length).ok()? <= COMMAND_BYTES)
                .then(|| redact::command(&bytes, executable.as_deref()))
                .flatten()
        });
        let parents = parent_chain(&self, |pid| {
            (fs::metadata(pid_path(pid)).ok()?.uid() == uid)
                .then(|| Self::read(pid).ok())
                .flatten()
        });
        let (browser, inspection_browser) = self.browser(&parents, executable.as_deref(), proxy);
        ProcessInfo {
            identity: self.identity,
            name: self.name,
            executable,
            uid,
            user: accounts.usernames.get(&uid).cloned(),
            started_at: accounts.start_time(self.identity.start_ticks),
            redacted_command,
            parents,
            browser,
            inspection_browser,
        }
    }

    fn browser(
        &self,
        parents: &[ProcessAncestor],
        executable: Option<&Path>,
        proxy: SocketAddr,
    ) -> (Option<ProcessAncestor>, bool) {
        let inspection_browser = {
            let browsers = BROWSERS.lock().unwrap_or_else(PoisonError::into_inner);
            let registered = parents
                .iter()
                .find(|parent| browsers.get(&parent.identity) == Some(&proxy));
            if let Some(parent) = registered {
                return (
                    Some(ProcessAncestor {
                        identity: parent.identity,
                        name: parent.name.clone(),
                    }),
                    true,
                );
            }
            browsers.get(&self.identity) == Some(&proxy)
        };
        if inspection_browser {
            return (None, true);
        }
        let family = executable
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .and_then(browser_family)
            .or_else(|| browser_family(&self.name));
        let browser = parents
            .iter()
            .rev()
            .find(|parent| {
                browser_family(&parent.name).is_some_and(|parent_family| {
                    family.is_none_or(|family| family == parent_family)
                })
            })
            .map(|parent| ProcessAncestor {
                identity: parent.identity,
                name: parent.name.clone(),
            });
        (browser, false)
    }
}

fn browser_family(name: &str) -> Option<&'static str> {
    match name {
        "chrome" | "chromium" | "chromium-browse" => Some("chrome"),
        "firefox" | "firefox-bin" | "firefox-esr" => Some("firefox"),
        _ => None,
    }
}

fn parent_chain(start: &ProcStat, read: impl Fn(u32) -> Option<ProcStat>) -> Vec<ProcessAncestor> {
    let mut parents = Vec::<ProcessAncestor>::new();
    let mut child = start.identity;
    let mut next = start.parent;
    for _ in 0..PARENT_LIMIT {
        if next == 0
            || next == start.identity.pid
            || parents.iter().any(|item| item.identity.pid == next)
        {
            break;
        }
        let Some(parent) = read(next) else {
            break;
        };
        if parent.identity.start_ticks > child.start_ticks {
            break;
        }
        child = parent.identity;
        next = parent.parent;
        parents.push(ProcessAncestor {
            identity: child,
            name: parent.name,
        });
    }
    parents
}

#[cfg(test)]
mod tests {
    use super::{
        PARENT_LIMIT, ProcStat, ProcessIdentity, ProcessStatus, has_socket, parent_chain, status,
    };
    use std::fs::File;
    use std::io;
    use std::os::unix::fs::symlink;

    #[test]
    fn selects_an_exact_socket_inode_link() -> io::Result<()> {
        let proc = tempfile::tempdir()?;
        std::fs::create_dir(proc.path().join("fd"))?;
        symlink("socket:[1234]", proc.path().join("fd/4"))?;
        symlink("socket:[91234]", proc.path().join("fd/6"))?;
        let process = File::open(proc.path())?;
        assert!(has_socket(&process, "socket:[1234]"));
        assert!(!has_socket(&process, "socket:[123]"));
        Ok(())
    }

    #[test]
    fn stat_handles_spaces_and_closing_parentheses_and_detects_pid_reuse() -> io::Result<()> {
        let line = format!("42 (weird ) name) S 7 {}1234 0", "0 ".repeat(17));
        let parsed =
            ProcStat::parse(&line, 42).ok_or_else(|| io::Error::other("failed parsing stat"))?;
        assert_eq!(parsed.parent, 7);
        assert_eq!(parsed.name, "weird ) name");
        assert_eq!(
            parsed.identity,
            ProcessIdentity {
                pid: 42,
                start_ticks: 1234
            }
        );

        let own = ProcStat::read(std::process::id())?.identity;
        assert_eq!(status(own), ProcessStatus::Running);
        assert_eq!(
            status(ProcessIdentity {
                start_ticks: own.start_ticks.saturating_add(1),
                ..own
            }),
            ProcessStatus::Exited
        );
        Ok(())
    }

    #[test]
    fn limits_parent_depth_and_rejects_a_parent_younger_than_the_child() {
        let process = |pid: u32| ProcStat {
            identity: ProcessIdentity {
                pid,
                start_ticks: u64::from(pid),
            },
            name: pid.to_string(),
            parent: pid.saturating_sub(1),
            exited: false,
        };
        let child = process(20);
        let parents = parent_chain(&child, |pid| Some(process(pid)));
        assert_eq!(parents.len(), PARENT_LIMIT);
        assert_eq!(parents.first().map(|parent| parent.identity.pid), Some(19));
        let reused = parent_chain(&child, |pid| {
            Some(ProcStat {
                identity: ProcessIdentity {
                    pid,
                    start_ticks: 21,
                },
                ..process(pid)
            })
        });
        assert!(reused.is_empty());
    }
}
