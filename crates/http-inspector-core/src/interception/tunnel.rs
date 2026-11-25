//! Relaying an upgraded CONNECT stream while counting bytes in each direction.

use std::fmt::Display;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf, copy_bidirectional};

use super::capture::{ERROR_BYTES, bounded_text};
use super::capture_context::TunnelCapture;
use http_inspector_exchange::{FailureStage, TunnelWriter};

/// Waits for the client upgrade, then relays bytes until both sides finish.
///
/// The outcome is written to `tunnel`: an upgrade error becomes an `Upgrade`
/// failure unless the proxy is shutting down, a relay error a `Tunnel`
/// failure, and a clean finish `Closed`. Because `tunnel` is already
/// constructed before this future is polled, dropping the future at any point
/// — including before its first poll — marks an unfinished tunnel `Incomplete`
/// and records its final snapshot in the session journal.
pub(super) async fn relay<F, C, E, U>(on_upgrade: F, mut upstream: U, tunnel: TunnelCapture)
where
    F: Future<Output = Result<C, E>>,
    C: AsyncRead + AsyncWrite + Unpin,
    E: Display,
    U: AsyncRead + AsyncWrite + Unpin,
{
    let writer = tunnel.writer();
    let client = match on_upgrade.await {
        Ok(client) => client,
        Err(error) => {
            if !tunnel.is_stopped() {
                writer.fail(FailureStage::Upgrade, failure_text(&error));
            }
            return;
        }
    };
    let mut client = CountingClient {
        inner: client,
        tunnel: writer,
    };
    match copy_bidirectional(&mut client, &mut upstream).await {
        Ok(_) => writer.close(),
        Err(error) => writer.fail(FailureStage::Tunnel, failure_text(&error)),
    }
}

fn failure_text(error: &impl Display) -> String {
    bounded_text(&error.to_string(), ERROR_BYTES)
}

/// Client side of a tunnel: reads go upstream, writes came from upstream.
struct CountingClient<'tunnel, C> {
    inner: C,
    tunnel: &'tunnel TunnelWriter,
}

impl<C: AsyncRead + Unpin> AsyncRead for CountingClient<'_, C> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buffer.filled().len();
        let result = Pin::new(&mut self.inner).poll_read(context, buffer);
        if matches!(result, Poll::Ready(Ok(()))) {
            let read = buffer.filled().len().saturating_sub(before);
            self.tunnel.add_client_to_upstream(read);
        }
        result
    }
}

impl<C: AsyncWrite + Unpin> AsyncWrite for CountingClient<'_, C> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let result = Pin::new(&mut self.inner).poll_write(context, data);
        if let Poll::Ready(Ok(written)) = result {
            self.tunnel.add_upstream_to_client(written);
        }
        result
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(context)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(context)
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::io;
    use std::pin::pin;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::task::{Context, Poll, Waker};

    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, duplex};

    use super::relay;
    use crate::interception::capture_context::{CaptureContext, TunnelCapture};
    use crate::recording::journal::FullLog;
    use http_inspector_exchange::{FailureStage, TunnelState};

    fn test_tunnel(log: Option<FullLog>, stopped: bool) -> (u64, TunnelCapture) {
        let capture = CaptureContext::for_test(log, Arc::new(AtomicBool::new(stopped)));
        let id = capture.id();
        (id, TunnelCapture::start(capture))
    }

    fn export_log(log: &FullLog) -> io::Result<String> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("tunnel.log");
        log.export(path.clone())?
            .recv()
            .map_err(io::Error::other)??;
        std::fs::read_to_string(path)
    }

    #[tokio::test]
    async fn records_an_upgrade_failure() {
        let (_, tunnel) = test_tunnel(None, false);
        let view = tunnel.preview();
        let (upstream, _far_upstream) = duplex(64);
        let on_upgrade = async { Err::<DuplexStream, _>("connection closed before upgrade") };
        relay(on_upgrade, upstream, tunnel).await;
        assert_eq!(
            view.snapshot().state(),
            &TunnelState::Failed {
                stage: FailureStage::Upgrade,
                message: "connection closed before upgrade".to_owned(),
            }
        );
    }

    #[tokio::test]
    async fn marks_upgrade_error_incomplete_when_proxy_is_stopping() -> io::Result<()> {
        let log = FullLog::new()?;
        let (id, tunnel) = test_tunnel(Some(log.clone()), true);
        let view = tunnel.preview();
        let (upstream, _far_upstream) = duplex(64);
        let on_upgrade = async { Err::<DuplexStream, _>("connection closed before upgrade") };
        relay(on_upgrade, upstream, tunnel).await;
        assert_eq!(view.snapshot().state(), &TunnelState::Incomplete);
        assert!(export_log(&log)?.contains(&format!("[{id}] Tunnel ended Incomplete")));
        Ok(())
    }

    #[test]
    fn marks_tunnel_incomplete_when_relay_future_is_dropped_before_first_poll() -> io::Result<()> {
        let log = FullLog::new()?;
        let (id, tunnel) = test_tunnel(Some(log.clone()), false);
        let view = tunnel.preview();
        let (upstream, _far_upstream) = duplex(64);
        let on_upgrade = std::future::pending::<Result<DuplexStream, &'static str>>();
        let future = relay(on_upgrade, upstream, tunnel);
        assert_eq!(view.snapshot().state(), &TunnelState::Open);

        drop(future);
        assert_eq!(view.snapshot().state(), &TunnelState::Incomplete);
        assert!(export_log(&log)?.contains(&format!("[{id}] Tunnel ended Incomplete")));
        Ok(())
    }

    #[test]
    fn marks_tunnel_incomplete_when_spawned_wrapper_is_dropped_before_first_poll() -> io::Result<()>
    {
        let log = FullLog::new()?;
        let (id, tunnel) = test_tunnel(Some(log.clone()), false);
        let view = tunnel.preview();
        let (upstream, _far_upstream) = duplex(64);
        let on_upgrade = std::future::pending::<Result<DuplexStream, &'static str>>();
        let task = async move {
            relay(on_upgrade, upstream, tunnel).await;
        };
        assert_eq!(view.snapshot().state(), &TunnelState::Open);

        drop(task);
        assert_eq!(view.snapshot().state(), &TunnelState::Incomplete);
        assert!(export_log(&log)?.contains(&format!("[{id}] Tunnel ended Incomplete")));
        Ok(())
    }

    #[test]
    fn marks_tunnel_incomplete_when_polled_relay_is_dropped_mid_upgrade() -> io::Result<()> {
        let log = FullLog::new()?;
        let (id, tunnel) = test_tunnel(Some(log.clone()), false);
        let view = tunnel.preview();
        let (upstream, _far_upstream) = duplex(64);
        let on_upgrade = std::future::pending::<Result<DuplexStream, &'static str>>();
        {
            let mut future = pin!(relay(on_upgrade, upstream, tunnel));
            let waker = Waker::noop();
            let mut context = Context::from_waker(waker);
            assert!(matches!(future.as_mut().poll(&mut context), Poll::Pending));
            assert_eq!(view.snapshot().state(), &TunnelState::Open);
        }
        assert_eq!(view.snapshot().state(), &TunnelState::Incomplete);
        assert!(export_log(&log)?.contains(&format!("[{id}] Tunnel ended Incomplete")));
        Ok(())
    }

    #[tokio::test]
    async fn counts_both_directions_until_close() -> io::Result<()> {
        let log = FullLog::new()?;
        let (id, tunnel) = test_tunnel(Some(log.clone()), false);
        let view = tunnel.preview();
        let (client, mut far_client) = duplex(64);
        let (upstream, mut far_upstream) = duplex(64);
        let on_upgrade = async { Ok::<_, &str>(client) };
        let relay_task = tokio::spawn(relay(on_upgrade, upstream, tunnel));

        far_client.write_all(b"ping!").await?;
        let mut request = [0_u8; 5];
        far_upstream.read_exact(&mut request).await?;
        far_upstream.write_all(b"pong").await?;
        let mut reply = [0_u8; 4];
        far_client.read_exact(&mut reply).await?;
        drop(far_client);
        drop(far_upstream);
        relay_task.await.map_err(io::Error::other)?;

        let snapshot = view.snapshot();
        assert_eq!(snapshot.state(), &TunnelState::Closed);
        assert_eq!(snapshot.client_to_upstream(), 5);
        assert_eq!(snapshot.upstream_to_client(), 4);
        let journal = export_log(&log)?;
        let ended_lines: Vec<_> = journal
            .lines()
            .filter(|line| line.starts_with(&format!("[{id}] Tunnel ended ")))
            .collect();
        assert_eq!(ended_lines.len(), 1);
        assert!(
            ended_lines
                .first()
                .is_some_and(|line| line.contains("Tunnel ended Closed"))
        );
        Ok(())
    }
}
