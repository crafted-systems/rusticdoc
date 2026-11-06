//! Ending connections that stop moving.
//!
//! Hyper's timer covers a request's headers, and the time limit on a route covers
//! its handler. Neither covers a client that stops reading a response: the write
//! blocks, and the connection holds its place for as long as the client likes.
//!
//! Time is only counted while hyper or a handler is actually blocked on the
//! client. [`StallGuard`] counts writes that stay pending, so a slow handler, with
//! nothing to write, never trips it. [`GuardedBody`] counts on the request body,
//! which is only polled when a handler wants more of it; reads on the socket
//! itself cannot tell, since hyper also polls them while a handler runs, to notice
//! a client going away.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::Bytes;
use http_body::{Body as HttpBody, Frame, SizeHint};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::{Instant, Sleep, sleep, sleep_until};

fn stalled() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "the client stopped moving")
}

/// A connection's stream, which fails a write that stays pending for too long.
pub struct StallGuard<S> {
    inner: S,
    limit: Duration,
    /// Runs while a write is pending, and is dropped by any progress.
    blocked: Option<Pin<Box<Sleep>>>,
}

impl<S> StallGuard<S> {
    pub fn new(inner: S, limit: Duration) -> Self {
        Self {
            inner,
            limit,
            blocked: None,
        }
    }

    fn watch<T>(
        &mut self,
        cx: &mut Context<'_>,
        polled: Poll<io::Result<T>>,
    ) -> Poll<io::Result<T>> {
        match polled {
            Poll::Ready(result) => {
                self.blocked = None;
                Poll::Ready(result)
            }
            Poll::Pending => {
                let limit = self.limit;
                let timer = self.blocked.get_or_insert_with(|| Box::pin(sleep(limit)));
                match timer.as_mut().poll(cx) {
                    Poll::Ready(()) => Poll::Ready(Err(stalled())),
                    Poll::Pending => Poll::Pending,
                }
            }
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for StallGuard<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for StallGuard<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let polled = Pin::new(&mut self.inner).poll_write(cx, buf);
        self.watch(cx, polled)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let polled = Pin::new(&mut self.inner).poll_write_vectored(cx, bufs);
        self.watch(cx, polled)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let polled = Pin::new(&mut self.inner).poll_flush(cx);
        self.watch(cx, polled)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let polled = Pin::new(&mut self.inner).poll_shutdown(cx);
        self.watch(cx, polled)
    }
}

/// A request body that fails when no bytes arrive for `stall` while a handler waits
/// for them, or when reading it takes longer than `total` in all.
pub struct GuardedBody<B> {
    inner: B,
    stall: Duration,
    total: Duration,
    /// Runs while the handler waits, and is dropped by any progress.
    blocked: Option<Pin<Box<Sleep>>>,
    /// Set by the first read.
    all: Option<Pin<Box<Sleep>>>,
}

impl<B> GuardedBody<B> {
    pub fn new(inner: B, stall: Duration, total: Duration) -> Self {
        Self {
            inner,
            stall,
            total,
            blocked: None,
            all: None,
        }
    }
}

impl<B> HttpBody for GuardedBody<B>
where
    B: HttpBody<Data = Bytes, Error = hyper::Error> + Unpin,
{
    type Data = Bytes;
    type Error = axum::BoxError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        let total = self.total;
        let this = &mut *self;
        let all = this
            .all
            .get_or_insert_with(|| Box::pin(sleep_until(Instant::now() + total)));
        if all.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Some(Err(stalled().into())));
        }
        match Pin::new(&mut this.inner).poll_frame(cx) {
            Poll::Ready(frame) => {
                this.blocked = None;
                Poll::Ready(frame.map(|frame| frame.map_err(Into::into)))
            }
            Poll::Pending => {
                let stall = this.stall;
                let timer = this.blocked.get_or_insert_with(|| Box::pin(sleep(stall)));
                match timer.as_mut().poll(cx) {
                    Poll::Ready(()) => Poll::Ready(Some(Err(stalled().into()))),
                    Poll::Pending => Poll::Pending,
                }
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}
