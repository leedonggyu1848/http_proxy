//! End-to-end command line flow against the real `http-proxy` executable:
//! start the daemon, send traffic through the proxy, wait for the record,
//! read it, save the session log, and stop. Runs without a display.

#![expect(
    clippy::indexing_slicing,
    reason = "indexing a serde_json value by key returns null instead of panicking"
)]

use std::fs;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;

use serde_json::Value;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const SECRET: &str = "cli-secret-token";
const COOKIE: &str = "session=cli-secret-cookie";
const BODY: &str = r#"{"data":{"items":[{"id":7,"name":"alpha"},{"id":9,"name":"beta"}]}}"#;

/// An isolated user environment: its own home, data, and runtime directories.
struct Sandbox {
    root: tempfile::TempDir,
    runtime: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> TestResult<Self> {
        // Unix socket paths are short, so the runtime directory lives in /tmp.
        let runtime = tempfile::Builder::new()
            .prefix("hi-cli")
            .tempdir_in("/tmp")?;
        fs::set_permissions(runtime.path(), fs::Permissions::from_mode(0o700))?;
        Ok(Self {
            root: tempfile::tempdir()?,
            runtime,
        })
    }

    fn command(&self, args: &[&str]) -> Command {
        let home = self.root.path().join("home");
        let mut command = Command::new(env!("CARGO_BIN_EXE_http-proxy"));
        command
            .args(args)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &home)
            .env("XDG_DATA_HOME", home.join("data"))
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_CACHE_HOME", home.join("cache"))
            .env("XDG_RUNTIME_DIR", self.runtime.path())
            .stdin(Stdio::null());
        command
    }

    fn run(&self, args: &[&str]) -> TestResult<Output> {
        Ok(self.command(args).output()?)
    }

    /// Runs a `--json` command that must succeed and returns its document.
    fn json(&self, args: &[&str]) -> TestResult<Value> {
        let output = self.run(args)?;
        let stdout = String::from_utf8(output.stdout)?;
        if !output.status.success() {
            return Err(format!("{args:?} failed: {stdout}").into());
        }
        let document: Value = serde_json::from_str(&stdout)?;
        if document.get("schema").and_then(Value::as_u64) != Some(1) {
            return Err(format!("{args:?} has no schema version: {stdout}").into());
        }
        Ok(document)
    }

    fn socket_directory(&self) -> PathBuf {
        self.runtime.path().join("http-inspector")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        // Never leave a daemon behind, even when an assertion failed.
        let _stopped = self.run(&["proxy", "stop"]);
    }
}

/// Serves one HTTP/1.1 response and returns the request it received.
fn serve_once(listener: TcpListener) -> thread::JoinHandle<std::io::Result<String>> {
    thread::spawn(move || {
        let (stream, _) = listener.accept()?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut head = String::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line)? == 0 || line == "\r\n" {
                break;
            }
            head.push_str(&line);
        }
        let mut writer = stream;
        write!(
            writer,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nSet-Cookie: id=cli-secret-set\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{BODY}",
            BODY.len()
        )?;
        writer.flush()?;
        Ok(head)
    })
}

fn get_through(proxy: SocketAddr, upstream: SocketAddr) -> TestResult<String> {
    let mut client = TcpStream::connect(proxy)?;
    write!(
        client,
        "GET http://{upstream}/api/users?page=2 HTTP/1.1\r\nHost: {upstream}\r\nAuthorization: Bearer {SECRET}\r\nCookie: {COOKIE}\r\nConnection: close\r\n\r\n"
    )?;
    let mut response = String::new();
    client.read_to_string(&mut response)?;
    Ok(response)
}

fn mode(path: &Path) -> TestResult<u32> {
    Ok(fs::symlink_metadata(path)?.permissions().mode() & 0o777)
}

fn assert_no_secret(label: &str, text: &str) {
    for secret in [SECRET, "cli-secret-cookie", "cli-secret-set"] {
        assert!(!text.contains(secret), "{label} leaked {secret}: {text}");
    }
}

fn start(sandbox: &Sandbox) -> TestResult<SocketAddr> {
    let missing = sandbox.run(&["proxy", "status", "--json"])?;
    assert_eq!(missing.status.code(), Some(3));
    let error: Value = serde_json::from_slice(&missing.stdout)?;
    assert_eq!(error["error"]["code"], "daemon_not_running");

    let started = sandbox.json(&["proxy", "start", "--port", "0", "--json"])?;
    assert_eq!(started["running"], true);
    assert_eq!(started["https_inspection"], false);
    let proxy: SocketAddr = started["address"].as_str().ok_or("no address")?.parse()?;
    assert!(proxy.ip().is_loopback());
    assert_eq!(mode(&sandbox.socket_directory())?, 0o700);
    assert_eq!(
        mode(&sandbox.socket_directory().join("control.sock"))?,
        0o600
    );

    let again = sandbox.json(&["proxy", "start", "--json"])?;
    assert_eq!(again["already_running"], true);
    assert_eq!(again["address"], started["address"]);
    Ok(proxy)
}

/// Sends one request through the proxy and waits for its record.
fn capture(sandbox: &Sandbox, proxy: SocketAddr) -> TestResult<String> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let upstream = listener.local_addr()?;
    let server = serve_once(listener);
    let response = get_through(proxy, upstream)?;
    assert!(response.ends_with(BODY), "{response}");
    let forwarded = server.join().map_err(|_| "server panicked")??;
    assert!(
        forwarded.contains(SECRET),
        "the proxy must forward credentials unchanged"
    );

    let waited = sandbox.json(&[
        "records",
        "wait",
        "--filter",
        "method:GET url:/api/users",
        "--timeout",
        "10s",
        "--json",
    ])?;
    let record = &waited["record"];
    assert_eq!(record["status"], 200);
    assert_eq!(record["settled"], true);
    Ok(record["id"].as_u64().ok_or("no id")?.to_string())
}

fn read_record(sandbox: &Sandbox, id: &str) -> TestResult {
    let listed = sandbox.json(&["records", "list", "--filter", "status:2xx", "--json"])?;
    assert_eq!(listed["matched"], 1);
    let none = sandbox.json(&["records", "list", "--kind", "failures", "--json"])?;
    assert_eq!(none["matched"], 0);

    let shown = sandbox.json(&["records", "show", id, "--json"])?;
    assert_eq!(shown["request"]["query"][0]["value"], "2");
    assert_eq!(shown["response"]["line"], "200 OK · HTTP/1.1");
    assert_no_secret("records show", &shown.to_string());

    let body = sandbox.json(&["records", "body", id, "--side", "response", "--json"])?;
    assert_eq!(body["format"], "JSON");
    assert!(
        body["text"]
            .as_str()
            .is_some_and(|text| text.contains("\n    \"items\""))
    );
    let item = sandbox.json(&[
        "records",
        "body",
        id,
        "--json-path",
        "$.data.items[1].name",
        "--json",
    ])?;
    assert_eq!(item["text"], "\"beta\"");
    Ok(())
}

fn check_exit_codes(sandbox: &Sandbox) -> TestResult {
    let missing_record = sandbox.run(&["records", "show", "999999", "--json"])?;
    assert_eq!(missing_record.status.code(), Some(4));
    let bad_filter = sandbox.run(&["records", "list", "--filter", "status:abc", "--json"])?;
    assert_eq!(bad_filter.status.code(), Some(2));
    let timeout = sandbox.run(&[
        "records",
        "wait",
        "--filter",
        "status:5xx",
        "--timeout",
        "200ms",
    ])?;
    assert_eq!(timeout.status.code(), Some(6));
    Ok(())
}

fn save_log(sandbox: &Sandbox) -> TestResult {
    let log = sandbox.root.path().join("session.log");
    let log_arg = log.to_str().ok_or("non-UTF-8 path")?;
    let saved = sandbox.json(&["log", "save", log_arg, "--json"])?;
    assert_eq!(saved["path"], log_arg);
    let text = fs::read_to_string(&log)?;
    assert!(text.contains("/api/users"), "{text}");
    assert_no_secret("session log", &text);
    let exists = sandbox.run(&["log", "save", log_arg, "--json"])?;
    assert_eq!(exists.status.code(), Some(1));
    Ok(())
}

fn stop(sandbox: &Sandbox, proxy: SocketAddr) -> TestResult {
    let cleared = sandbox.json(&["records", "clear", "--json"])?;
    assert_eq!(cleared["removed"], 1);
    let stopped = sandbox.json(&["proxy", "stop", "--json"])?;
    assert_eq!(stopped["stopped"], true);
    let after = sandbox.run(&["proxy", "status"])?;
    assert_eq!(after.status.code(), Some(3));
    assert!(
        TcpStream::connect(proxy).is_err(),
        "the proxy must stop listening"
    );
    Ok(())
}

#[test]
fn proxy_records_and_log_work_without_a_display() -> TestResult {
    let sandbox = Sandbox::new()?;
    let proxy = start(&sandbox)?;
    let id = capture(&sandbox, proxy)?;
    read_record(&sandbox, &id)?;
    check_exit_codes(&sandbox)?;
    save_log(&sandbox)?;
    stop(&sandbox, proxy)
}

#[test]
fn refuses_a_control_directory_other_users_can_open() -> TestResult {
    let sandbox = Sandbox::new()?;
    let directory = sandbox.socket_directory();
    fs::create_dir(&directory)?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o777))?;
    let output = sandbox.run(&["proxy", "start", "--port", "0", "--json"])?;
    assert_eq!(output.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(error["error"]["code"], "insecure_control_path");
    Ok(())
}

#[test]
fn help_and_schema_need_no_daemon() -> TestResult {
    let sandbox = Sandbox::new()?;
    let help = sandbox.run(&["--help"])?;
    assert!(help.status.success());
    assert!(String::from_utf8(help.stdout)?.contains("records"));
    let schema = sandbox.json(&["schema"])?;
    assert!(schema["commands"].as_array().is_some_and(|commands| {
        commands
            .iter()
            .any(|command| command["command"] == "records wait")
    }));
    let usage = sandbox.run(&["records", "body"])?;
    assert_eq!(usage.status.code(), Some(2));
    Ok(())
}
