//! `http-proxy shell` and `http-proxy exec`: run a program with the
//! inspection environment, wait for it, and remove the CA bundle after.
//!
//! The program is a child of this process, so the bundle lives exactly as
//! long as it does. Interrupts from the terminal reach the child directly
//! and are ignored here; hang-up and terminate are passed on to the child.

use std::ffi::{OsStr, OsString};
use std::io;
use std::net::SocketAddr;
use std::os::unix::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::time::Duration;

use http_inspector_core::shell::{self, NoProxy, ShellEnvironment, SystemRoots};
use rustix::process::{Pid, Signal};
use serde_json::Value;
use tokio::signal::unix::{SignalKind, signal};

use crate::args::{ExecArgs, InspectionArgs, ShellArgs};
use crate::client;
use crate::control::ControlPath;
use crate::error::{CliError, ErrorCode};
use crate::output::{self, Output};
use crate::protocol::Request;

/// How long a child may take to exit after a forwarded hang-up or
/// terminate before it is killed. Interactive shells ignore `SIGTERM`.
const EXIT_GRACE: Duration = Duration::from_secs(5);
/// Exit code for a command that exists but cannot run, as in POSIX shells.
pub(crate) const NOT_EXECUTABLE: u8 = 126;
/// Exit code for a command that was not found, as in POSIX shells.
pub(crate) const NOT_FOUND: u8 = 127;

/// Arguments after the program name that make `http-proxy` open an
/// inspection shell for a proxy it does not manage, such as the desktop
/// app's. `ca_directory` turns on the CA variables.
#[must_use]
pub fn terminal_arguments(proxy: SocketAddr, ca_directory: Option<&Path>) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["shell", "--hold-on-error", "--proxy"]
        .map(OsString::from)
        .into();
    args.push(proxy.to_string().into());
    if let Some(directory) = ca_directory {
        args.push("--ca-directory".into());
        args.push(directory.as_os_str().to_owned());
    }
    args
}

/// Runs `$SHELL` interactively and returns its exit code.
pub(crate) fn shell(
    args: &ShellArgs,
    control: &ControlPath,
    output: Output,
) -> Result<u8, CliError> {
    let prepared =
        target(&args.options, control).and_then(|target| environment(&args.options, &target));
    let environment = match prepared {
        Ok(environment) => environment,
        Err(error) if args.options.hold_on_error => {
            output.error(&error);
            hold();
            return Ok(error.code.exit_code());
        }
        Err(error) => return Err(error),
    };
    output::notice(&banner(&environment));
    let program = std::env::var_os("SHELL")
        .filter(|shell| Path::new(shell).is_absolute())
        .unwrap_or_else(|| OsString::from("/bin/sh"));
    run(
        &environment,
        &program,
        &[],
        output,
        args.options.hold_on_error,
    )
}

/// Keeps a terminal window opened by the app until the error is read.
fn hold() {
    output::notice("Press Enter to close this window.\n");
    let _closed = io::stdin().read_line(&mut String::new()).is_err();
}

/// Runs one command and returns its exit code.
pub(crate) fn exec(args: &ExecArgs, control: &ControlPath, output: Output) -> Result<u8, CliError> {
    let target = target(&args.options, control)?;
    let environment = environment(&args.options, &target)?;
    let (program, rest) = args
        .command
        .split_first()
        .ok_or_else(|| CliError::new(ErrorCode::Usage, "exec needs a command"))?;
    run(
        &environment,
        program,
        rest,
        output,
        args.options.hold_on_error,
    )
}

/// The proxy to use and, when HTTPS is decrypted, the CA directory.
struct Target {
    proxy: SocketAddr,
    ca_directory: Option<PathBuf>,
}

fn target(options: &InspectionArgs, control: &ControlPath) -> Result<Target, CliError> {
    if let Some(proxy) = options.proxy {
        return Ok(Target {
            proxy,
            ca_directory: options.ca_directory.clone(),
        });
    }
    let status = client::call(control, &Request::Status)?;
    let proxy = proxy_address(&status)?;
    let https = status.get("https_inspection").and_then(Value::as_bool) == Some(true);
    let ca_directory = if https {
        Some(crate::local::ca_directory()?)
    } else {
        None
    };
    Ok(Target {
        proxy,
        ca_directory,
    })
}

/// The address of a running proxy from a `status` reply.
pub(crate) fn proxy_address(status: &Value) -> Result<SocketAddr, CliError> {
    status
        .get("address")
        .and_then(Value::as_str)
        .and_then(|address| address.parse().ok())
        .ok_or_else(|| {
            CliError::new(ErrorCode::ProxyNotRunning, "The proxy is not running")
                .with_hint("run `http-proxy proxy start`")
        })
}

fn environment(options: &InspectionArgs, target: &Target) -> Result<ShellEnvironment, CliError> {
    let no_proxy = if options.keep_no_proxy {
        NoProxy::Keep
    } else {
        NoProxy::Clear
    };
    let environment = ShellEnvironment::new(target.proxy, no_proxy).map_err(|error| {
        CliError::new(ErrorCode::ProxyNotRunning, error.to_string())
            .with_hint("run `http-proxy proxy start`")
    })?;
    let Some(directory) = &target.ca_directory else {
        return Ok(environment);
    };
    let ca = crate::local::load_public_ca(directory)?;
    environment
        .with_temporary_bundle(&ca, &shell::private_parent(|name| std::env::var_os(name)))
        .map_err(|error| CliError::new(ErrorCode::HttpsUnavailable, error.to_string()))
}

/// The lines printed when an interactive shell starts.
fn banner(environment: &ShellEnvironment) -> String {
    let mut text = format!(
        "HTTP Inspector shell: programs started here use the proxy at {}.\n",
        environment.proxy()
    );
    match environment.bundle() {
        Some(bundle) => {
            text.push_str(
                "HTTPS is decrypted; the local CA is trusted next to the system roots.\n",
            );
            if bundle.system_roots() == &SystemRoots::Missing {
                text.push_str(
                    "No system CA bundle was found: HTTPS that bypasses the proxy fails certificate checks.\n",
                );
            }
        }
        None => {
            text.push_str("HTTPS inspection is off: HTTPS passes through as CONNECT tunnels.\n");
        }
    }
    text.push_str(
        "These variables keep this address: if the proxy stops or changes port, exit and open a new shell.\n",
    );
    text
}

/// Runs the program to completion and returns its exit code. The
/// environment, and with it the CA bundle, is dropped after the child exits.
/// With `hold_on_error`, a program that cannot start waits for Enter.
fn run(
    environment: &ShellEnvironment,
    program: &OsStr,
    args: &[OsString],
    output: Output,
    hold_on_error: bool,
) -> Result<u8, CliError> {
    let mut command = std::process::Command::new(program);
    command.args(args);
    environment.apply(&mut command);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| CliError::io("Could not start the command runner", &error))?;
    match runtime.block_on(supervise(command)) {
        Ok(status) => Ok(exit_code(status)),
        Err(Failure::Spawn(error)) => {
            let code = match error.kind() {
                io::ErrorKind::NotFound => NOT_FOUND,
                _ => NOT_EXECUTABLE,
            };
            output.error(&CliError::io(
                &format!("Could not run {}", program.to_string_lossy()),
                &error,
            ));
            if hold_on_error {
                hold();
            }
            Ok(code)
        }
        Err(Failure::Wait(error)) => Err(CliError::io("Could not wait for the command", &error)),
    }
}

enum Failure {
    Spawn(io::Error),
    Wait(io::Error),
}

async fn supervise(command: std::process::Command) -> Result<ExitStatus, Failure> {
    // Handlers are installed before the child starts, so no signal can end
    // this process without removing the bundle.
    let mut interrupt = signal(SignalKind::interrupt()).map_err(Failure::Wait)?;
    let mut quit = signal(SignalKind::quit()).map_err(Failure::Wait)?;
    let mut hangup = signal(SignalKind::hangup()).map_err(Failure::Wait)?;
    let mut terminate = signal(SignalKind::terminate()).map_err(Failure::Wait)?;
    let mut child = tokio::process::Command::from(command)
        .spawn()
        .map_err(Failure::Spawn)?;
    let pid = child
        .id()
        .and_then(|id| i32::try_from(id).ok())
        .and_then(Pid::from_raw);
    let mut deadline = None;
    loop {
        tokio::select! {
            status = child.wait() => return status.map_err(Failure::Wait),
            _ = interrupt.recv() => {}
            _ = quit.recv() => {}
            _ = hangup.recv() => deadline = forward(pid, Signal::HUP, deadline),
            _ = terminate.recv() => deadline = forward(pid, Signal::TERM, deadline),
            () = sleep_until(deadline), if deadline.is_some() => {
                child.kill().await.map_err(Failure::Wait)?;
            }
        }
    }
}

/// Passes `signal` to the child and starts the grace period, once.
fn forward(
    pid: Option<Pid>,
    signal: Signal,
    deadline: Option<tokio::time::Instant>,
) -> Option<tokio::time::Instant> {
    if let Some(pid) = pid {
        let _gone = rustix::process::kill_process(pid, signal).is_err();
    }
    deadline.or_else(|| Some(tokio::time::Instant::now() + EXIT_GRACE))
}

async fn sleep_until(deadline: Option<tokio::time::Instant>) {
    if let Some(deadline) = deadline {
        tokio::time::sleep_until(deadline).await;
    }
}

/// The child's exit code, or 128 plus the signal that ended it.
fn exit_code(status: ExitStatus) -> u8 {
    status.code().map_or_else(
        || {
            let signal = status.signal().unwrap_or(0);
            u8::try_from(128 + signal).unwrap_or(u8::MAX)
        },
        |code| u8::try_from(code & 0xff).unwrap_or(u8::MAX),
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use clap::Parser as _;

    use super::*;
    use crate::args::{Cli, Command};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn inspection(proxy: &str, ca_directory: Option<&Path>) -> Result<InspectionArgs, CliError> {
        let mut args = vec![OsString::from("http-proxy")];
        args.extend(terminal_arguments(
            proxy
                .parse()
                .map_err(|_| CliError::new(ErrorCode::Usage, "address"))?,
            ca_directory,
        ));
        match Cli::try_parse_from(args).map(|cli| cli.command) {
            Ok(Command::Shell(shell)) => Ok(shell.options),
            _ => Err(CliError::new(ErrorCode::Usage, "not a shell command")),
        }
    }

    #[test]
    fn terminal_arguments_parse_back_into_the_shell_command() -> TestResult {
        let ca = Path::new("/home/user/.local/share/http-inspector/ca");
        let options = inspection("127.0.0.1:18080", Some(ca))?;
        assert_eq!(options.proxy, Some("127.0.0.1:18080".parse()?));
        assert_eq!(options.ca_directory.as_deref(), Some(ca));
        assert!(options.hold_on_error);
        assert!(!options.keep_no_proxy);
        Ok(())
    }

    /// Runs `script` through `exec` with the given proxy and CA directory.
    fn exec_script(script: &str, ca_directory: Option<&Path>) -> Result<u8, CliError> {
        let mut options = inspection("127.0.0.1:18080", ca_directory)?;
        // Holding would wait for Enter on the test's stdin.
        options.hold_on_error = false;
        let args = ExecArgs {
            options,
            command: ["/bin/sh", "-c", script].map(OsString::from).into(),
        };
        exec(
            &args,
            &ControlPath::in_directory(PathBuf::from("/nonexistent")),
            Output::new(false),
        )
    }

    #[test]
    fn exec_preserves_exit_codes_and_sets_the_proxy() -> TestResult {
        assert_eq!(exec_script("exit 7", None)?, 7);
        assert_eq!(
            exec_script(
                "[ \"$https_proxy\" = http://127.0.0.1:18080 ] && [ -z \"${NO_PROXY+x}\" ] && [ -z \"${GIT_SSL_CAINFO+x}\" ]",
                None
            )?,
            0
        );
        assert_eq!(exec_script("kill -TERM $$", None)?, 128 + 15);
        let mut options = inspection("127.0.0.1:18080", None)?;
        options.hold_on_error = false;
        let missing = ExecArgs {
            options,
            command: vec![OsString::from("/nonexistent/command")],
        };
        let control = ControlPath::in_directory(PathBuf::from("/nonexistent"));
        assert_eq!(exec(&missing, &control, Output::new(true))?, NOT_FOUND);
        Ok(())
    }

    #[test]
    fn exec_bundle_is_private_and_removed_after_the_command() -> TestResult {
        let root = tempfile::tempdir()?;
        let ca_directory = root.path().join("ca");
        let record = root.path().join("bundle");
        // The shell reads only the public certificate: no identity needed.
        http_inspector_core::https::CaInfo::load_or_create(&ca_directory)?;
        fs::remove_file(ca_directory.join("ca-identity.pem"))?;
        let script = format!(
            "printf '%s' \"$SSL_CERT_FILE\" > '{}'; [ \"$GIT_SSL_CAINFO\" = \"$SSL_CERT_FILE\" ] && [ \"$NODE_EXTRA_CA_CERTS\" = '{}' ] && ! grep -q 'PRIVATE KEY' \"$SSL_CERT_FILE\" && [ \"$(ls -l \"$SSL_CERT_FILE\" | cut -c1-10)\" = -rw------- ] && [ \"$(ls -ld \"$(dirname \"$SSL_CERT_FILE\")\" | cut -c1-10)\" = drwx------ ]",
            record.display(),
            ca_directory.join("root-ca.pem").display(),
        );
        assert_eq!(exec_script(&script, Some(&ca_directory))?, 0);
        let bundle = PathBuf::from(fs::read_to_string(&record)?);
        assert!(bundle.ends_with(shell::BUNDLE_FILE));
        assert!(!bundle.exists());
        assert!(!bundle.parent().is_some_and(Path::exists));
        Ok(())
    }

    #[test]
    fn missing_ca_fails_without_creating_one() {
        let root = tempfile::tempdir().ok();
        let ca_directory = root.as_ref().map(|root| root.path().join("ca"));
        let result = exec_script("exit 0", ca_directory.as_deref());
        assert_eq!(
            result.err().map(|error| error.code),
            Some(ErrorCode::HttpsUnavailable)
        );
        assert!(!ca_directory.is_some_and(|directory| directory.exists()));
    }

    #[test]
    fn stopped_proxy_is_reported() {
        let error = proxy_address(&serde_json::json!({ "address": null })).err();
        assert_eq!(
            error.map(|error| error.code),
            Some(ErrorCode::ProxyNotRunning)
        );
    }
}
