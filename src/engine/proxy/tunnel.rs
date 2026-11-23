//! Relaying an upgraded CONNECT stream while counting bytes in each direction.

use std::fmt::Display;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf, copy_bidirectional};

use crate::engine::capture::{ERROR_BYTES, bounded_text};
use crate::exchange::{FailureStage, TunnelPreview};

/// Waits for the client upgrade, then relays bytes until both sides finish.
///
/// The outcome is written to `tunnel`: an upgrade error becomes an `Upgrade`
/// failure, a relay error a `Tunnel` failure, and a clean finish `Closed`.
/// When the relay is dropped before it finishes, which happens when the
/// proxy stops, the tunnel becomes `Incomplete`.
pub(super) async fn relay<F, C, E, U>(on_upgrade: F, mut upstream: U, tunnel: TunnelPreview)
where
    F: Future<Output = Result<C, E>>,
    C: AsyncRead + AsyncWrite + Unpin,
    E: Display,
    U: AsyncRead + AsyncWrite + Unpin,
{
    let _abandoned_on_drop = AbandonOnDrop(tunnel.clone());
    let client = match on_upgrade.await {
        Ok(client) => client,
        Err(error) => {
            tunnel.fail(FailureStage::Upgrade, failure_text(&error));
            return;
        }
    };
    let mut client = CountingClient {
        inner: client,
        tunnel: tunnel.clone(),
    };
    match copy_bidirectional(&mut client, &mut upstream).await {
        Ok(_) => tunnel.close(),
        Err(error) => tunnel.fail(FailureStage::Tunnel, failure_text(&error)),
    }
}

/// Marks the tunnel `Incomplete` unless it already ended.
struct AbandonOnDrop(TunnelPreview);

impl Drop for AbandonOnDrop {
    fn drop(&mut self) {
        self.0.abandon();
    }
}

fn failure_text(error: &impl Display) -> String {
    bounded_text(&error.to_string(), ERROR_BYTES)
}

/// Client side of a tunnel: reads go upstream, writes came from upstream.
struct CountingClient<C> {
    inner: C,
    tunnel: TunnelPreview,
}

impl<C: AsyncRead + Unpin> AsyncRead for CountingClient<C> {
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

impl<C: AsyncWrite + Unpin> AsyncWrite for CountingClient<C> {
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, duplex};

    use super::*;
    use crate::exchange::TunnelState;

    #[tokio::test]
    async fn records_an_upgrade_failure() {
        let tunnel = TunnelPreview::new();
        let (upstream, _far_upstream) = duplex(64);
        let on_upgrade = async { Err::<DuplexStream, _>("connection closed before upgrade") };
        relay(on_upgrade, upstream, tunnel.clone()).await;
        assert_eq!(
            tunnel.snapshot().state(),
            &TunnelState::Failed {
                stage: FailureStage::Upgrade,
                message: "connection closed before upgrade".to_owned(),
            }
        );
    }

    #[tokio::test]
    async fn counts_both_directions_until_close() -> io::Result<()> {
        let tunnel = TunnelPreview::new();
        let (client, mut far_client) = duplex(64);
        let (upstream, mut far_upstream) = duplex(64);
        let on_upgrade = async { Ok::<_, &str>(client) };
        let relay_task = tokio::spawn(relay(on_upgrade, upstream, tunnel.clone()));

        far_client.write_all(b"ping!").await?;
        let mut request = [0_u8; 5];
        far_upstream.read_exact(&mut request).await?;
        far_upstream.write_all(b"pong").await?;
        let mut reply = [0_u8; 4];
        far_client.read_exact(&mut reply).await?;
        drop(far_client);
        drop(far_upstream);
        relay_task.await.map_err(io::Error::other)?;

        let snapshot = tunnel.snapshot();
        assert_eq!(snapshot.state(), &TunnelState::Closed);
        assert_eq!(snapshot.client_to_upstream(), 5);
        assert_eq!(snapshot.upstream_to_client(), 4);
        Ok(())
    }
}
