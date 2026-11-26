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
        let root = tempfile::tempdir()?;
        fs::create_dir(root.path().join("tmp"))?;
        Ok(Self { root, runtime })
    }

    /// The daemon's `TMPDIR`, where nothing may remain.
    fn temporary(&self) -> PathBuf {
        self.root.path().join("tmp")
    }

    fn assert_temporary_empty(&self) -> TestResult {
        let left: Vec<_> = fs::read_dir(self.temporary())?
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .collect();
        assert!(left.is_empty(), "temporary files left behind: {left:?}");
        Ok(())
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
            .env("TMPDIR", self.temporary())
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
    thread::spawn(move || answer(&listener))
}

/// Serves `count` requests and returns their heads and bodies.
fn serve(listener: TcpListener, count: usize) -> thread::JoinHandle<std::io::Result<Vec<String>>> {
    thread::spawn(move || (0..count).map(|_| answer(&listener)).collect())
}

/// Accepts one connection, reads its request including a `Content-Length`
/// body, and answers with [`BODY`].
fn answer(listener: &TcpListener) -> std::io::Result<String> {
    let (stream, _) = listener.accept()?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request = String::new();
    let mut length = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
        request.push_str(&line);
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    request.push_str(&String::from_utf8_lossy(&body));
    let mut writer = stream;
    write!(
        writer,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nSet-Cookie: id=cli-secret-set\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{BODY}",
        BODY.len()
    )?;
    writer.flush()?;
    Ok(request)
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
    assert!(!sandbox.socket_directory().join("control.sock").exists());
    sandbox.assert_temporary_empty()
}

#[test]
fn proxy_records_and_log_work_without_a_display() -> TestResult {
    let sandbox = Sandbox::new()?;
    let proxy = start(&sandbox)?;
    let id = capture(&sandbox, proxy)?;
    read_record(&sandbox, &id)?;
    // The session log holds bodies but has no name on disk.
    sandbox.assert_temporary_empty()?;
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

#[test]
fn concurrent_starts_share_one_daemon() -> TestResult {
    let sandbox = Sandbox::new()?;
    let children = (0..4)
        .map(|_| {
            sandbox
                .command(&["proxy", "start", "--port", "0", "--json"])
                .stdout(Stdio::piped())
                .spawn()
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut documents = Vec::new();
    for child in children {
        let output = child.wait_with_output()?;
        let stdout = String::from_utf8(output.stdout)?;
        assert!(output.status.success(), "{stdout}");
        documents.push(serde_json::from_str::<Value>(&stdout)?);
    }
    let started = documents
        .iter()
        .filter(|document| document["already_running"] != true)
        .count();
    assert_eq!(started, 1, "{documents:?}");
    let first = documents.first().ok_or("no document")?;
    for document in &documents {
        assert_eq!(document["address"], first["address"]);
        assert_eq!(document["pid"], first["pid"]);
    }
    let stopped = sandbox.json(&["proxy", "stop", "--json"])?;
    assert_eq!(stopped["stopped"], true);
    wait_for_exit(&first["pid"])
}

#[test]
fn start_refuses_settings_other_than_the_running_proxy() -> TestResult {
    let sandbox = Sandbox::new()?;
    let started = sandbox.json(&["proxy", "start", "--port", "0", "--json"])?;
    let port = started["port"].as_u64().ok_or("no port")?;
    let other = if port == 65_535 { port - 1 } else { port + 1 }.to_string();
    for args in [
        ["proxy", "start", "--https-inspection", "--json"].as_slice(),
        ["proxy", "start", "--port", &other, "--json"].as_slice(),
    ] {
        let output = sandbox.run(args)?;
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        let error: Value = serde_json::from_slice(&output.stdout)?;
        assert_eq!(error["error"]["code"], "proxy_conflict", "{args:?}");
    }
    let unchanged = sandbox.json(&["proxy", "status", "--json"])?;
    assert_eq!(unchanged["https_inspection"], false);
    assert_eq!(unchanged["port"], port);
    let same = sandbox.json(&["proxy", "start", "--port", &port.to_string(), "--json"])?;
    assert_eq!(same["already_running"], true);
    Ok(())
}

#[test]
fn a_terminated_daemon_leaves_no_session_log_and_can_be_restarted() -> TestResult {
    let sandbox = Sandbox::new()?;
    let proxy = start(&sandbox)?;
    let _id = capture(&sandbox, proxy)?;
    let status = sandbox.json(&["proxy", "status", "--json"])?;
    let pid = status["pid"].as_u64().ok_or("no pid")?.to_string();
    assert!(
        Command::new("kill")
            .args(["-TERM", &pid])
            .status()?
            .success()
    );
    wait_for_exit(&status["pid"])?;
    sandbox.assert_temporary_empty()?;
    // The socket file is stale now; the next start replaces it.
    let restarted = sandbox.json(&["proxy", "start", "--port", "0", "--json"])?;
    assert_eq!(restarted["already_running"], false);
    let stopped = sandbox.json(&["proxy", "stop", "--json"])?;
    assert_eq!(stopped["stopped"], true);
    sandbox.assert_temporary_empty()
}

/// Waits up to five seconds for a daemon process to exit.
fn wait_for_exit(pid: &Value) -> TestResult {
    let pid = pid.as_u64().ok_or("no pid")?.to_string();
    for _ in 0..100 {
        let alive = Command::new("kill")
            .args(["-0", &pid])
            .stderr(Stdio::null())
            .status()?
            .success();
        if !alive {
            return Ok(());
        }
        pause();
    }
    Err(format!("process {pid} did not exit").into())
}

#[expect(
    clippy::disallowed_methods,
    reason = "the test polls another process, not async work"
)]
fn pause() {
    thread::sleep(std::time::Duration::from_millis(50));
}

#[test]
fn env_and_ca_info_describe_the_proxy_without_the_private_key() -> TestResult {
    let sandbox = Sandbox::new()?;
    let started = sandbox.json(&["proxy", "start", "--port", "0", "--json"])?;
    let proxy_url = format!(
        "http://{}",
        started["address"].as_str().ok_or("no address")?
    );
    let env = sandbox.json(&["proxy", "env", "--ca-bundle", "--json"])?;
    let names: Vec<&str> = env["variables"]
        .as_array()
        .map_or_else(Vec::new, |variables| {
            variables
                .iter()
                .filter_map(|v| v["name"].as_str())
                .collect()
        });
    // Without HTTPS inspection there is no CA to export.
    assert_eq!(
        names,
        ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"]
    );
    assert_eq!(env["variables"][2]["value"], proxy_url.as_str());
    let text = String::from_utf8(sandbox.run(&["proxy", "env"])?.stdout)?;
    assert!(
        text.contains(&format!("export http_proxy='{proxy_url}'")),
        "{text}"
    );

    let ca = sandbox.run(&["ca", "info", "--json"])?;
    assert!(ca.status.success());
    let ca_text = String::from_utf8(ca.stdout)?;
    assert!(
        !ca_text.contains("PRIVATE KEY") && !ca_text.contains("ca-identity"),
        "{ca_text}"
    );
    let ca: Value = serde_json::from_str(&ca_text)?;
    let certificate = PathBuf::from(ca["certificate_path"].as_str().ok_or("no path")?);
    assert!(fs::read_to_string(&certificate)?.starts_with("-----BEGIN CERTIFICATE-----"));
    assert_eq!(
        ca["fingerprint_sha256"]
            .as_str()
            .map(|text| text.split(':').count()),
        Some(32)
    );
    assert!(ca["valid_from"].as_str() < ca["valid_until"].as_str());

    sandbox.json(&["proxy", "set", "--https-inspection", "on", "--json"])?;
    let inspected = sandbox.json(&["proxy", "env", "--ca-bundle", "--json"])?;
    let exported: Vec<(&str, bool)> =
        inspected["variables"]
            .as_array()
            .map_or_else(Vec::new, |variables| {
                variables
                    .iter()
                    .filter(|v| v["value"] == ca["certificate_path"])
                    .filter_map(|v| {
                        Some((v["name"].as_str()?, v["replaces_system_roots"].as_bool()?))
                    })
                    .collect()
            });
    assert_eq!(
        exported,
        [
            ("NODE_EXTRA_CA_CERTS", false),
            ("SSL_CERT_FILE", true),
            ("CURL_CA_BUNDLE", true),
            ("REQUESTS_CA_BUNDLE", true),
        ]
    );
    Ok(())
}

#[test]
fn replay_and_curl_reproduce_a_capture_without_its_credentials() -> TestResult {
    let sandbox = Sandbox::new()?;
    let proxy = start(&sandbox)?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let upstream = listener.local_addr()?;
    let server = serve(listener, 3);
    get_through(proxy, upstream)?;
    // The record is listed at once, without waiting for it first.
    let immediate = sandbox.json(&["records", "list", "--json"])?;
    assert_eq!(immediate["matched"], 1);
    let waited = sandbox.json(&["records", "wait", "--filter", "url:/api/users", "--json"])?;
    let id = waited["record"]["id"].as_u64().ok_or("no id")?.to_string();

    check_list_rows(&sandbox)?;
    check_curl(&sandbox, &id)?;
    check_replay_refusals(&sandbox, &id)?;
    check_replays(&sandbox, &id)?;
    let received = server.join().map_err(|_| "server panicked")??;
    let [_, replay, post] = received.as_slice() else {
        return Err(format!("expected 3 requests, got {received:?}").into());
    };
    assert!(
        replay.to_ascii_lowercase().contains("x-debug: 1"),
        "{replay}"
    );
    assert!(!replay.contains(SECRET), "{replay}");
    assert!(
        post.starts_with("POST ") && post.ends_with(r#"{"name":"gamma"}"#),
        "{post}"
    );
    Ok(())
}

/// The default list row has only the summary fields; `--full` has all.
fn check_list_rows(sandbox: &Sandbox) -> TestResult {
    let listed = sandbox.json(&["records", "list", "--json"])?;
    let row = listed["records"][0].as_object().ok_or("no row")?;
    let mut keys: Vec<&str> = row.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "duration_ms",
            "id",
            "method",
            "outcome",
            "request_bytes",
            "response_bytes",
            "status",
            "url"
        ]
    );
    let full = sandbox.json(&["records", "list", "--full", "--json"])?;
    assert_eq!(full["records"][0]["summary"], "200 OK");
    Ok(())
}

/// A dry run prints the request only; a replay and a confirmed POST send.
fn check_replays(sandbox: &Sandbox, id: &str) -> TestResult {
    let dry = sandbox.json(&[
        "records",
        "replay",
        id,
        "--remove-header",
        "Authorization",
        "--remove-header",
        "Cookie",
        "--header",
        "X-Debug: 1",
        "--dry-run",
        "--json",
    ])?;
    assert_eq!(dry["dry_run"], true);
    assert!(dry.get("record").is_none());
    assert!(dry["request"]["headers"].as_array().is_some_and(|headers| {
        headers
            .iter()
            .any(|h| h["name"] == "x-debug" && h["value"] == "1")
            && !headers.iter().any(|h| h["name"] == "authorization")
    }));
    let unchanged = sandbox.json(&["records", "list", "--json"])?;
    assert_eq!(unchanged["matched"], 1, "a dry run must not send");

    let replayed = sandbox.json(&[
        "records",
        "replay",
        id,
        "--remove-header",
        "Authorization",
        "--remove-header",
        "Cookie",
        "--header",
        "X-Debug: 1",
        "--json",
    ])?;
    assert_eq!(replayed["record"]["replay_of"].to_string(), id);
    assert_eq!(replayed["record"]["status"], 200);
    assert_ne!(replayed["record"]["id"].to_string(), id);

    let body = sandbox.root.path().join("body.json");
    fs::write(&body, r#"{"name":"gamma"}"#)?;
    let posted = sandbox.json(&[
        "records",
        "replay",
        id,
        "--method",
        "POST",
        "--remove-header",
        "Authorization",
        "--remove-header",
        "Cookie",
        "--body-file",
        body.to_str().ok_or("path")?,
        "--yes",
        "--json",
    ])?;
    assert_eq!(posted["record"]["method"], "POST");
    assert_eq!(posted["request"]["body_bytes"], 16);
    Ok(())
}

fn check_curl(sandbox: &Sandbox, id: &str) -> TestResult {
    let curl = sandbox.json(&["records", "curl", id, "--json"])?;
    let command = curl["command"].as_str().ok_or("no command")?;
    assert!(
        command.starts_with("curl ") && command.contains("/api/users?page=2"),
        "{command}"
    );
    assert_no_secret("records curl", command);
    assert!(
        !command.to_ascii_lowercase().contains("authorization"),
        "{command}"
    );
    assert_eq!(
        curl["redacted_headers"],
        serde_json::json!(["authorization", "cookie"])
    );
    let placeholders = sandbox.json(&["records", "curl", id, "--placeholders", "--json"])?;
    assert!(
        placeholders["command"]
            .as_str()
            .is_some_and(|text| text.contains("<REDACTED>"))
    );
    Ok(())
}

fn check_replay_refusals(sandbox: &Sandbox, id: &str) -> TestResult {
    // Redacted credentials cannot be sent as captured.
    let redacted = sandbox.run(&["records", "replay", id, "--json"])?;
    assert_eq!(redacted.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&redacted.stdout)?;
    assert_eq!(error["error"]["code"], "not_reproducible");
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("authorization"))
    );
    // A state-changing method needs --yes.
    let unconfirmed = sandbox.run(&[
        "records",
        "replay",
        id,
        "--method",
        "DELETE",
        "--remove-header",
        "Authorization",
        "--remove-header",
        "Cookie",
        "--json",
    ])?;
    assert_eq!(unconfirmed.status.code(), Some(5));
    let error: Value = serde_json::from_slice(&unconfirmed.stdout)?;
    assert_eq!(error["error"]["code"], "confirmation_required");
    let listed = sandbox.json(&["records", "list", "--json"])?;
    assert_eq!(listed["matched"], 1, "a refused replay must not send");
    Ok(())
}
