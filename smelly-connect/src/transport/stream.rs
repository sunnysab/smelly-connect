use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pub trait AsyncStream: AsyncRead + AsyncWrite + Send + Unpin + 'static {}

impl<T> AsyncStream for T where T: AsyncRead + AsyncWrite + Send + Unpin + 'static {}

pub struct VpnStream {
    inner: Box<dyn AsyncStream>,
}

impl VpnStream {
    pub fn new<T>(inner: T) -> Self
    where
        T: AsyncStream,
    {
        Self {
            inner: Box::new(inner),
        }
    }
}

impl AsyncRead for VpnStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut *self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for VpnStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut *self.inner).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut *self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut *self.inner).poll_shutdown(cx)
    }
}
