//! A copied `curl` command, run by a real shell, repeats the captured request.

use std::error::Error;
use std::net::SocketAddr;
use std::process::Command;
use std::time::Duration;

use http_proxy::{
    BodyLifecycle, Exchange, ProxyConfig, ProxyHandle, RedactedHeaders, curl_command,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time::timeout;

type TestError = Box<dyn Error + Send + Sync>;

const TEST_TIMEOUT: Duration = Duration::from_secs(10);

/// What the test server received for one request.
#[derive(Debug, Eq, PartialEq)]
struct Received {
    method: String,
    target: String,
    /// Lowercase names with values, sorted, so header order does not matter.
    headers: Vec<(String, Vec<u8>)>,
    body: Vec<u8>,
}

#[tokio::test]
async fn copied_commands_send_the_captured_text_request_again() -> Result<(), TestError> {
    let body = "name=it's $HOME\n\\path 한글\ttab".as_bytes();
    repeat_through_curl(body, "text/plain; charset=utf-8").await
}

#[tokio::test]
async fn copied_commands_send_unprintable_bodies_byte_for_byte() -> Result<(), TestError> {
    let body = b"\x1f\x8b\0\x01'$\\%\r\n\xed\x95\x9c\xff@";
    repeat_through_curl(body, "application/octet-stream").await
}

#[expect(
    clippy::print_stderr,
    reason = "A skipped run must say why it passed without running curl"
)]
async fn repeat_through_curl(body: &[u8], content_type: &str) -> Result<(), TestError> {
    if Command::new("curl").arg("--version").output().is_err() {
        eprintln!("skipping: curl is not installed");
        return Ok(());
    }
    let (server, mut received) = test_server().await?;
    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
    let mut request = format!(
        "PUT http://{server}/echo/%24x?q='a'&k=%ED%95%9C HTTP/1.1\r\n\
         Host: {server}\r\n\
         User-Agent: test-agent/1.0\r\n\
         Accept: */*\r\n\
         X-Quote: it's $HOME \\n `id` \"q\"\r\n\
         X-Korean: 한글 값\r\n\
         X-Empty:\r\n\
         X-Repeated: one\r\n\
         X-Repeated: two\r\n\
         Accept-Encoding: gzip, deflate\r\n\
         Cookie: session=secret\r\n\
         Content-Type: {content_type}\r\n\
         Proxy-Connection: keep-alive\r\n\
         Connection: close\r\n\
         Content-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    request.extend_from_slice(body);
    let mut client = TcpStream::connect(proxy.local_addr()).await?;
    client.write_all(&request).await?;
    let mut response = Vec::new();
    timeout(TEST_TIMEOUT, client.read_to_end(&mut response)).await??;
    assert!(response.starts_with(b"HTTP/1.1 200"));
    let original = timeout(TEST_TIMEOUT, received.recv())
        .await?
        .ok_or("server stopped")?;

    let exchange = timeout(TEST_TIMEOUT, proxy.receive())
        .await?
        .ok_or("capture channel closed")?;
    wait_for_request_body(&exchange).await?;
    let command = curl_command(&exchange, RedactedHeaders::Omit)?;
    assert_eq!(command.redacted_headers(), ["cookie"]);
    assert!(!command.text().contains("secret"));

    // Run the shell off the runtime so the test server can answer it.
    let text = command.text().to_owned();
    let output = tokio::task::spawn_blocking(move || {
        Command::new("sh")
            .arg("-c")
            .arg(text)
            .env_remove("http_proxy")
            .env_remove("HTTP_PROXY")
            .env_remove("https_proxy")
            .env_remove("HTTPS_PROXY")
            .env_remove("all_proxy")
            .env_remove("ALL_PROXY")
            .env("CURL_HOME", "/nonexistent")
            .output()
    })
    .await??;
    assert!(
        output.status.success(),
        "curl failed: {}\n{}",
        String::from_utf8_lossy(&output.stderr),
        command.text()
    );
    assert_eq!(output.stdout, b"ok");
    let repeated = timeout(TEST_TIMEOUT, received.recv())
        .await?
        .ok_or("server stopped")?;

    // The redacted Cookie is the one intended difference.
    let without_cookie = |received: &Received| Received {
        method: received.method.clone(),
        target: received.target.clone(),
        headers: received
            .headers
            .iter()
            .filter(|(name, _)| name != "cookie")
            .cloned()
            .collect(),
        body: received.body.clone(),
    };
    assert!(original.headers.iter().any(|(name, _)| name == "cookie"));
    assert_eq!(without_cookie(&repeated), without_cookie(&original));
    assert_eq!(repeated.body, body);
    proxy.stop()?;
    Ok(())
}

async fn wait_for_request_body(exchange: &Exchange) -> Result<(), TestError> {
    let poll = async {
        loop {
            if let Some(snapshot) = exchange
                .request()
                .body()
                .and_then(http_proxy::BodyPreview::snapshot)
                && snapshot.lifecycle() != BodyLifecycle::Streaming
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    Ok(timeout(TEST_TIMEOUT, poll).await?)
}

/// Starts a server that records every request and answers `ok`.
async fn test_server() -> std::io::Result<(SocketAddr, mpsc::Receiver<Received>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (sender, receiver) = mpsc::channel(4);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let sender = sender.clone();
            tokio::spawn(async move {
                if let Ok(received) = read_request(&mut stream).await {
                    let _sent = sender.send(received).await;
                    let _written = stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                        )
                        .await;
                }
            });
        }
    });
    Ok((address, receiver))
}

/// Reads one HTTP/1.1 request with a `Content-Length` body.
async fn read_request(stream: &mut TcpStream) -> std::io::Result<Received> {
    let invalid = |message: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, message);
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        head.push(stream.read_u8().await?);
        if head.len() > 64 * 1024 {
            return Err(invalid("request head too large"));
        }
    }
    let mut lines = head
        .split(|&byte| byte == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
        .filter(|line| !line.is_empty());
    let request_line = String::from_utf8_lossy(lines.next().ok_or_else(|| invalid("empty"))?);
    let mut parts = request_line.split(' ');
    let method = parts.next().unwrap_or_default().to_owned();
    let target = parts.next().unwrap_or_default().to_owned();
    let mut headers = Vec::new();
    let mut length = 0;
    for line in lines {
        let colon = line
            .iter()
            .position(|&byte| byte == b':')
            .ok_or_else(|| invalid("header without colon"))?;
        let (name, value) = line.split_at(colon);
        let name = String::from_utf8_lossy(name).to_ascii_lowercase();
        let value = value.get(1..).unwrap_or_default().trim_ascii().to_vec();
        if name == "content-length" {
            length = String::from_utf8_lossy(&value)
                .parse()
                .map_err(|_| invalid("bad length"))?;
        }
        headers.push((name, value));
    }
    headers.sort();
    let mut body = vec![0; length];
    stream.read_exact(&mut body).await?;
    Ok(Received {
        method,
        target,
        headers,
        body,
    })
}
