use std::fs;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc::{self, SyncSender};
use std::time::Duration;

use tempfile::TempDir;
use tokio::process::Command;
use tokio::sync::oneshot;

use super::{Browser, BrowserEvent, BrowserSession, Kind};
use crate::{CaInfo, CaTrustStore};

type LaunchResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub(super) fn open(
    browser: Browser,
    proxy: SocketAddr,
    ca_directory: Option<PathBuf>,
) -> Result<BrowserSession, std::io::Error> {
    let (sender, events) = mpsc::sync_channel(2);
    let (stop, receiver) = oneshot::channel();
    let worker = std::thread::Builder::new()
        .name("inspection-browser".to_owned())
        .spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| error.to_string())
                .and_then(|runtime| {
                    runtime
                        .block_on(run(
                            &browser,
                            proxy,
                            ca_directory.as_deref(),
                            receiver,
                            &sender,
                        ))
                        .map_err(|error| error.to_string())
                });
            let _closed = sender.send(BrowserEvent::Finished(result)).is_err();
        })?;
    Ok(BrowserSession {
        events,
        stop: Some(stop),
        worker: Some(worker),
    })
}

async fn run(
    browser: &Browser,
    proxy: SocketAddr,
    ca_directory: Option<&Path>,
    mut stop: oneshot::Receiver<()>,
    events: &SyncSender<BrowserEvent>,
) -> LaunchResult<()> {
    let profile = tokio::select! {
        result = prepare(browser, proxy, ca_directory) => result?,
        _ = &mut stop => return Ok(()),
    };
    let mut child = command(browser, proxy, profile.path()).spawn()?;
    // A missing package or sandbox failure should appear as an error, not "open".
    tokio::select! {
        status = child.wait() => return Err(format!("{} exited before opening a window ({})", browser.name, status?).into()),
        () = tokio::time::sleep(Duration::from_millis(700)) => {},
        _ = &mut stop => { child.kill().await?; return Ok(()); },
    }
    let _closed = events.send(BrowserEvent::Opened).is_err();
    let status = tokio::select! {
        result = child.wait() => result?,
        _ = &mut stop => { child.kill().await?; return Ok(()); },
    };
    if !status.success() {
        return Err(format!("{} exited with {status}", browser.name).into());
    }
    Ok(())
}

async fn prepare(
    browser: &Browser,
    proxy: SocketAddr,
    ca_directory: Option<&Path>,
) -> LaunchResult<TempDir> {
    if !proxy.ip().is_loopback() || proxy.port() == 0 {
        return Err("Browser inspection requires a listening loopback proxy".into());
    }
    let profile = profile_directory(browser.kind)?;
    if matches!(browser.kind, Kind::Firefox { .. }) {
        fs::write(profile.path().join("user.js"), firefox_preferences(proxy))?;
    }
    if let Some(directory) = ca_directory {
        let ca = CaInfo::load_or_create(directory)?;
        let store = match browser.kind {
            Kind::Firefox { .. } => CaTrustStore::for_firefox_profile(profile.path())?,
            Kind::Chrome => CaTrustStore::for_current_user()?,
        };
        store.install(&ca).await?;
    }
    Ok(profile)
}

fn profile_directory(kind: Kind) -> LaunchResult<TempDir> {
    let mut builder = tempfile::Builder::new();
    builder
        .prefix("http-inspector-browser-")
        .permissions(fs::Permissions::from_mode(0o700));
    if kind == (Kind::Firefox { snap: true }) {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or("Firefox Snap requires an absolute HOME")?;
        let common = home.join("snap/firefox/common");
        fs::create_dir_all(&common)?;
        Ok(builder.tempdir_in(common)?)
    } else {
        Ok(builder.tempdir()?)
    }
}

fn firefox_preferences(proxy: SocketAddr) -> String {
    format!(
        r#"user_pref("network.proxy.type", 1);
user_pref("network.proxy.http", "{host}");
user_pref("network.proxy.http_port", {port});
user_pref("network.proxy.ssl", "{host}");
user_pref("network.proxy.ssl_port", {port});
user_pref("network.proxy.no_proxies_on", "");
user_pref("network.proxy.allow_hijacking_localhost", true);
user_pref("network.http.http3.enable", false);
user_pref("network.proxy.failover_direct", false);
user_pref("browser.shell.checkDefaultBrowser", false);
user_pref("browser.startup.page", 0);
user_pref("browser.startup.homepage_override.mstone", "ignore");
"#,
        host = proxy.ip(),
        port = proxy.port()
    )
}

fn command(browser: &Browser, proxy: SocketAddr, profile: &Path) -> Command {
    let mut command = Command::new(&browser.executable);
    match browser.kind {
        Kind::Firefox { .. } => {
            command
                .args(["--no-remote", "--profile"])
                .arg(profile)
                .args(["--private-window", "about:blank"]);
        }
        Kind::Chrome => {
            let mut data_directory = std::ffi::OsString::from("--user-data-dir=");
            data_directory.push(profile);
            command
                .arg(data_directory)
                .arg("--incognito")
                .arg(format!("--proxy-server=http://{proxy}"))
                .args([
                    "--proxy-bypass-list=<-loopback>",
                    "--disable-quic",
                    "--no-first-run",
                    "--no-default-browser-check",
                    "about:blank",
                ]);
        }
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    command
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::CaTrustStatus;

    fn firefox(executable: PathBuf) -> Browser {
        Browser {
            name: "Test Firefox",
            executable,
            kind: Kind::Firefox { snap: false },
        }
    }

    #[tokio::test]
    async fn firefox_profile_trusts_only_public_ca_and_is_removed() -> LaunchResult<()> {
        let directory = tempfile::tempdir()?;
        let ca_directory = directory.path().join("ca");
        let browser = firefox(PathBuf::from("/unused"));
        let proxy = "127.0.0.1:18765".parse()?;
        let profile = prepare(&browser, proxy, Some(&ca_directory)).await?;
        let path = profile.path().to_owned();
        let ca = CaInfo::load_or_create(&ca_directory)?;
        assert_eq!(
            CaTrustStore::for_firefox_profile(&path)?
                .status(&ca)
                .await?,
            CaTrustStatus::Trusted
        );
        assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o700);
        assert!(!path.join("ca-identity.pem").exists());
        let prefs = fs::read_to_string(path.join("user.js"))?;
        assert!(prefs.contains("network.proxy.http_port\", 18765"));
        assert!(prefs.contains("network.proxy.ssl_port\", 18765"));
        assert!(prefs.contains("network.proxy.allow_hijacking_localhost\", true"));
        drop(profile);
        assert!(!path.exists());
        assert!(ca.certificate_path().exists());
        Ok(())
    }

    async fn next_event(session: &mut BrowserSession) -> LaunchResult<BrowserEvent> {
        Ok(tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(event) = session.try_receive() {
                    break event;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await?)
    }

    #[tokio::test]
    async fn private_launch_owns_process_and_profile_until_closed() -> LaunchResult<()> {
        let directory = tempfile::tempdir()?;
        let executable = directory.path().join("test browser");
        fs::write(
            &executable,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.args\"\nprintf '%s' \"$$\" > \"$0.pid\"\nexec sleep 60\n",
        )?;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))?;
        let browser = firefox(executable.clone());
        let mut session = browser.open("127.0.0.1:18765".parse()?, None)?;
        assert!(matches!(
            next_event(&mut session).await?,
            BrowserEvent::Opened
        ));
        let pid = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(pid) = fs::read_to_string(executable.with_extension("pid"))
                    && !pid.is_empty()
                {
                    break pid;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await?;
        let args = fs::read_to_string(executable.with_extension("args"))?;
        let mut args = args.lines();
        assert_eq!(args.next(), Some("--no-remote"));
        assert_eq!(args.next(), Some("--profile"));
        let profile = PathBuf::from(args.next().ok_or("profile missing")?);
        assert_eq!(
            args.collect::<Vec<_>>(),
            ["--private-window", "about:blank"]
        );
        assert!(profile.join("user.js").exists());
        assert!(
            std::process::Command::new("kill")
                .args(["-0", &pid])
                .status()?
                .success()
        );
        drop(session);
        assert!(
            !std::process::Command::new("kill")
                .args(["-0", &pid])
                .status()?
                .success()
        );
        assert!(!profile.exists());
        Ok(())
    }

    #[tokio::test]
    async fn startup_failure_is_reported_instead_of_opened() -> LaunchResult<()> {
        let browser = firefox(PathBuf::from("/bin/false"));
        let mut session = browser.open("127.0.0.1:18765".parse()?, None)?;
        assert!(matches!(
            next_event(&mut session).await?,
            BrowserEvent::Finished(Err(_))
        ));
        assert!(
            prepare(&browser, "192.0.2.1:8080".parse()?, None)
                .await
                .is_err()
        );
        assert!(
            prepare(&browser, "127.0.0.1:0".parse()?, None)
                .await
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn chrome_preserves_profile_path_and_enables_private_proxy() {
        use std::ffi::{OsStr, OsString};
        use std::os::unix::ffi::OsStringExt;
        let profile = PathBuf::from(OsString::from_vec(b"/tmp/inspection \xff".to_vec()));
        let browser = Browser {
            name: "Chrome",
            executable: PathBuf::from("/unused"),
            kind: Kind::Chrome,
        };
        let proxy = SocketAddr::from(([127, 0, 0, 1], 8080));
        let command = command(&browser, proxy, &profile);
        let args: Vec<_> = command.as_std().get_args().collect();
        let mut expected = OsString::from("--user-data-dir=");
        expected.push(&profile);
        assert_eq!(args.first().copied(), Some(expected.as_os_str()));
        for flag in [
            "--incognito",
            "--proxy-server=http://127.0.0.1:8080",
            "--proxy-bypass-list=<-loopback>",
            "--disable-quic",
        ] {
            assert!(args.contains(&OsStr::new(flag)));
        }
    }
}
