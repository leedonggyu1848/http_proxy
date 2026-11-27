//! External terminal emulators that run one command in a new window.
//!
//! The desktop app opens an inspection shell this way. The command is the
//! app's own executable, so the shell's environment and CA bundle are made
//! and removed by that process, however the terminal starts it.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::Duration;

use tokio::process::Command;

use super::discovery::find_executable;

/// How long a terminal may take to fail before it counts as open. Many
/// terminals hand the window to a server process and exit at once.
const STARTUP: Duration = Duration::from_millis(1000);

/// Supported terminals in preference order after `$TERMINAL`: the Debian
/// alternative first, since it names the user's choice.
const CANDIDATES: [(&str, &str, Style); 9] = [
    ("x-terminal-emulator", "x-terminal-emulator", Style::DashE),
    ("gnome-terminal", "GNOME Terminal", Style::DoubleDash),
    ("konsole", "Konsole", Style::DashE),
    ("xfce4-terminal", "Xfce Terminal", Style::DashX),
    ("kitty", "kitty", Style::Plain),
    ("alacritty", "Alacritty", Style::DashE),
    ("wezterm", "WezTerm", Style::WeztermStart),
    ("foot", "foot", Style::Plain),
    ("xterm", "XTerm", Style::DashE),
];

/// How a terminal takes the command to run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Style {
    /// `terminal -e program args…`
    DashE,
    /// `terminal -- program args…`
    DoubleDash,
    /// `terminal -x program args…`
    DashX,
    /// `terminal program args…`
    Plain,
    /// `wezterm start -- program args…`
    WeztermStart,
    /// macOS Terminal.app through `osascript`.
    #[cfg_attr(
        not(target_os = "macos"),
        expect(dead_code, reason = "Terminal.app is discovered only on macOS")
    )]
    AppleScript,
}

/// An installed terminal emulator.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Terminal {
    name: String,
    executable: PathBuf,
    style: Style,
}

/// Reason a terminal did not open.
#[derive(Debug, thiserror::Error)]
pub enum TerminalLaunchError {
    #[error("No terminal emulator found; set TERMINAL to one")]
    NotFound,
    #[error("{terminal} exited before opening a window ({status})")]
    ExitedBeforeOpen {
        terminal: String,
        status: ExitStatus,
    },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// The outcome of a launch, reported once by a background worker.
pub struct TerminalLaunch {
    events: Receiver<Result<(), TerminalLaunchError>>,
}

impl Terminal {
    /// The terminal to use: `$TERMINAL` when it names an executable, then
    /// the first installed candidate.
    #[must_use]
    pub fn find() -> Option<Self> {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let paths: Vec<PathBuf> = std::env::split_paths(&path).collect();
        let preferred = std::env::var_os("TERMINAL").and_then(|name| from_variable(&name, &paths));
        preferred
            .or_else(|| discover(&paths))
            .or_else(apple_terminal)
    }

    /// The terminal name for messages.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Runs `program` with `args` in a new terminal window. The terminal is
    /// not closed when the launch is dropped.
    pub fn open(&self, program: &Path, args: &[OsString]) -> std::io::Result<TerminalLaunch> {
        let terminal = self.clone();
        let command_args = arguments(self.style, program, args);
        let (sender, events) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("inspection-terminal".to_owned())
            .spawn(move || {
                let result = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(TerminalLaunchError::from)
                    .and_then(|runtime| runtime.block_on(run(&terminal, &command_args, &sender)));
                if let Err(error) = result {
                    let _closed = sender.send(Err(error)).is_err();
                }
            })?;
        Ok(TerminalLaunch { events })
    }
}

impl TerminalLaunch {
    /// The launch result once it is known, without blocking.
    #[must_use]
    pub fn try_receive(&mut self) -> Option<Result<(), TerminalLaunchError>> {
        self.events.try_recv().ok()
    }
}

/// Starts the terminal, reports whether it survived startup, then waits
/// for it so it does not linger as a zombie.
async fn run(
    terminal: &Terminal,
    args: &[OsString],
    events: &SyncSender<Result<(), TerminalLaunchError>>,
) -> Result<(), TerminalLaunchError> {
    let mut child = Command::new(&terminal.executable)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    tokio::select! {
        status = child.wait() => {
            let status = status?;
            if !status.success() {
                return Err(TerminalLaunchError::ExitedBeforeOpen {
                    terminal: terminal.name.clone(),
                    status,
                });
            }
            let _closed = events.send(Ok(())).is_err();
            return Ok(());
        }
        () = tokio::time::sleep(STARTUP) => {}
    }
    let _closed = events.send(Ok(())).is_err();
    let _status = child.wait().await;
    Ok(())
}

/// `$TERMINAL` as a path or a command on `paths`. Known names keep their
/// own argument style; others get `-e`, which most terminals accept.
fn from_variable(value: &OsStr, paths: &[PathBuf]) -> Option<Terminal> {
    let value = Path::new(value);
    let executable = if value.is_absolute() {
        find_executable(&[value.parent()?.to_owned()], value.file_name()?.to_str()?)
    } else {
        find_executable(paths, value.to_str()?)
    }?;
    let command = executable.file_name()?.to_string_lossy().into_owned();
    let (name, style) = CANDIDATES
        .iter()
        .find(|(candidate, _, _)| *candidate == command)
        .map_or_else(
            || (command.clone(), Style::DashE),
            |&(_, name, style)| (name.to_owned(), style),
        );
    Some(Terminal {
        name,
        executable,
        style,
    })
}

fn discover(paths: &[PathBuf]) -> Option<Terminal> {
    CANDIDATES.iter().find_map(|&(command, name, style)| {
        find_executable(paths, command).map(|executable| Terminal {
            name: name.to_owned(),
            executable,
            style,
        })
    })
}

/// Terminal.app, driven through `osascript`.
#[cfg(target_os = "macos")]
fn apple_terminal() -> Option<Terminal> {
    let installed = [
        "/System/Applications/Utilities/Terminal.app",
        "/Applications/Utilities/Terminal.app",
    ]
    .iter()
    .any(|path| Path::new(path).exists());
    let executable = find_executable(&[PathBuf::from("/usr/bin")], "osascript")?;
    installed.then(|| Terminal {
        name: "Terminal".to_owned(),
        executable,
        style: Style::AppleScript,
    })
}

#[cfg(not(target_os = "macos"))]
const fn apple_terminal() -> Option<Terminal> {
    None
}

fn arguments(style: Style, program: &Path, args: &[OsString]) -> Vec<OsString> {
    let command = std::iter::once(program.as_os_str().to_owned()).chain(args.iter().cloned());
    let prefix: &[&str] = match style {
        Style::DashE => &["-e"],
        Style::DoubleDash => &["--"],
        Style::DashX => &["-x"],
        Style::Plain => &[],
        Style::WeztermStart => &["start", "--"],
        Style::AppleScript => return apple_script(program, args),
    };
    prefix.iter().map(OsString::from).chain(command).collect()
}

/// `osascript` arguments that open a Terminal.app window running the
/// command, quoted for the shell and then for `AppleScript`.
fn apple_script(program: &Path, args: &[OsString]) -> Vec<OsString> {
    let shell = std::iter::once(program.as_os_str())
        .chain(args.iter().map(OsString::as_os_str))
        .map(|part| format!("'{}'", part.to_string_lossy().replace('\'', r"'\''")))
        .collect::<Vec<_>>()
        .join(" ");
    let quoted = shell.replace('\\', r"\\").replace('"', "\\\"");
    [
        "-e",
        "tell application \"Terminal\" to activate",
        "-e",
        &format!("tell application \"Terminal\" to do script \"{quoted}\""),
    ]
    .into_iter()
    .map(OsString::from)
    .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

    fn executable(directory: &Path, name: &str, script: &str) -> std::io::Result<PathBuf> {
        let path = directory.join(name);
        fs::write(&path, script)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        Ok(path)
    }

    #[test]
    fn prefers_terminal_variable_then_candidates_in_order() -> TestResult {
        let directory = tempfile::tempdir()?;
        let paths = [directory.path().to_owned()];
        assert_eq!(discover(&paths), None);
        executable(directory.path(), "xterm", "#!/bin/sh\n")?;
        let kitty = executable(directory.path(), "kitty", "#!/bin/sh\n")?;
        let custom = executable(directory.path(), "my-term", "#!/bin/sh\n")?;
        assert_eq!(
            discover(&paths).map(|terminal| terminal.style),
            Some(Style::Plain)
        );
        let named = from_variable(OsStr::new("my-term"), &paths).ok_or("not found")?;
        assert_eq!((named.name(), named.style), ("my-term", Style::DashE));
        let absolute = from_variable(kitty.as_os_str(), &[]).ok_or("not found")?;
        assert_eq!((absolute.name(), absolute.style), ("kitty", Style::Plain));
        assert_eq!(absolute.executable, kitty);
        assert!(from_variable(OsStr::new("missing"), &paths).is_none());
        assert!(custom.exists());
        Ok(())
    }

    #[test]
    fn builds_each_argument_style() {
        let program = Path::new("/usr/bin/http-proxy");
        let args = [OsString::from("shell"), OsString::from("--proxy")];
        let render = |style| {
            arguments(style, program, &args)
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(" ")
        };
        assert_eq!(render(Style::DashE), "-e /usr/bin/http-proxy shell --proxy");
        assert_eq!(
            render(Style::DoubleDash),
            "-- /usr/bin/http-proxy shell --proxy"
        );
        assert_eq!(render(Style::DashX), "-x /usr/bin/http-proxy shell --proxy");
        assert_eq!(render(Style::Plain), "/usr/bin/http-proxy shell --proxy");
        assert_eq!(
            render(Style::WeztermStart),
            "start -- /usr/bin/http-proxy shell --proxy"
        );
        let script = apple_script(Path::new("/Apps/It's \"here\""), &args);
        assert_eq!(
            script.get(3).map(|arg| arg.to_string_lossy().into_owned()),
            Some(
                r#"tell application "Terminal" to do script "'/Apps/It'\\''s \"here\"' 'shell' '--proxy'""#
                    .to_owned()
            )
        );
    }

    async fn result(launch: &mut TerminalLaunch) -> Result<(), TerminalLaunchError> {
        loop {
            if let Some(result) = launch.try_receive() {
                return result;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn reports_a_terminal_that_fails_at_startup_and_passes_the_command() -> TestResult {
        let directory = tempfile::tempdir()?;
        let failing = Terminal {
            name: "Broken".to_owned(),
            executable: PathBuf::from("/bin/false"),
            style: Style::DashE,
        };
        let mut launch = failing.open(Path::new("/bin/true"), &[])?;
        let outcome = tokio::time::timeout(Duration::from_secs(5), result(&mut launch)).await?;
        assert!(matches!(
            outcome,
            Err(TerminalLaunchError::ExitedBeforeOpen { .. })
        ));
        let recorder = executable(
            directory.path(),
            "recorder",
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.args\"\n",
        )?;
        let working = Terminal {
            name: "Recorder".to_owned(),
            executable: recorder.clone(),
            style: Style::DoubleDash,
        };
        let mut launch = working.open(Path::new("/opt/app"), &[OsString::from("shell")])?;
        tokio::time::timeout(Duration::from_secs(5), result(&mut launch)).await??;
        let args = fs::read_to_string(recorder.with_extension("args"))?;
        assert_eq!(args, "--\n/opt/app\nshell\n");
        Ok(())
    }
}
