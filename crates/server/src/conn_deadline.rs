//! Closes connections that never send their first request byte.
//!
//! hyper's header timeout only starts once a request begins, so a client
//! that connects and stays silent would hold its socket forever. The
//! acceptor here wraps each accepted connection (after TLS, when enabled)
//! so its first read fails with `TimedOut` if no byte arrives in time.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use axum_server::accept::Accept;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::Sleep;

/// Wraps another acceptor with a first-byte deadline.
#[derive(Debug, Clone)]
pub struct FirstByteDeadline<A> {
    inner: A,
    timeout: Duration,
}

impl<A> FirstByteDeadline<A> {
    /// Connections from `inner` must send a byte within `timeout`.
    pub fn new(inner: A, timeout: Duration) -> Self {
        Self { inner, timeout }
    }
}

impl<I, S, A> Accept<I, S> for FirstByteDeadline<A>
where
    A: Accept<I, S>,
    A::Future: Send + 'static,
{
    type Stream = DeadlineStream<A::Stream>;
    type Service = A::Service;
    type Future = Pin<Box<dyn Future<Output = io::Result<(Self::Stream, Self::Service)>> + Send>>;

    fn accept(&self, stream: I, service: S) -> Self::Future {
        let accepted = self.inner.accept(stream, service);
        let timeout = self.timeout;
        Box::pin(async move {
            let (stream, service) = accepted.await?;
            Ok((DeadlineStream::new(stream, timeout), service))
        })
    }
}

/// A stream whose reads fail until the first byte if `timeout` elapses.
pub struct DeadlineStream<S> {
    inner: S,
    deadline: Option<Pin<Box<Sleep>>>,
}

impl<S> DeadlineStream<S> {
    fn new(inner: S, timeout: Duration) -> Self {
        Self {
            inner,
            deadline: Some(Box::pin(tokio::time::sleep(timeout))),
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for DeadlineStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        match Pin::new(&mut self.inner).poll_read(cx, buf) {
            Poll::Ready(result) => {
                if buf.filled().len() > before {
                    self.deadline = None;
                }
                Poll::Ready(result)
            }
            Poll::Pending => {
                let expired = self
                    .deadline
                    .as_mut()
                    .is_some_and(|deadline| deadline.as_mut().poll(cx).is_ready());
                if expired {
                    let error = io::Error::new(io::ErrorKind::TimedOut, "no request received");
                    return Poll::Ready(Err(error));
                }
                Poll::Pending
            }
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for DeadlineStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test(start_paused = true)]
    async fn a_silent_connection_times_out() {
        let (_client, server) = tokio::io::duplex(64);
        let mut stream = DeadlineStream::new(server, Duration::from_secs(30));
        let mut buf = [0u8; 8];
        let error = stream.read(&mut buf).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }

    #[tokio::test(start_paused = true)]
    async fn the_deadline_ends_with_the_first_byte() {
        let (mut client, server) = tokio::io::duplex(64);
        let mut stream = DeadlineStream::new(server, Duration::from_secs(30));
        client.write_all(b"G").await.unwrap();
        let mut buf = [0u8; 8];
        assert_eq!(stream.read(&mut buf).await.unwrap(), 1);
        tokio::time::advance(Duration::from_secs(60)).await;
        client.write_all(b"ET").await.unwrap();
        assert_eq!(stream.read(&mut buf).await.unwrap(), 2);
    }
}
