use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use tokio::net::UdpSocket;

pub trait AsyncDatagramSocket: Send + Sync + 'static {
    fn send_to<'a>(
        &'a self,
        data: &'a [u8],
        target: SocketAddr,
    ) -> Pin<Box<dyn Future<Output = io::Result<usize>> + Send + 'a>>;

    fn recv_from<'a>(
        &'a self,
        buf: &'a mut [u8],
    ) -> Pin<Box<dyn Future<Output = io::Result<(usize, SocketAddr)>> + Send + 'a>>;

    fn local_addr(&self) -> io::Result<SocketAddr>;
}

#[derive(Clone)]
pub struct VpnUdpSocket {
    inner: Arc<dyn AsyncDatagramSocket>,
}

impl VpnUdpSocket {
    pub fn new<T>(inner: T) -> Self
    where
        T: AsyncDatagramSocket,
    {
        Self {
            inner: Arc::new(inner),
        }
    }

    pub async fn send_to(&self, data: &[u8], target: SocketAddr) -> io::Result<usize> {
        self.inner.send_to(data, target).await
    }

    pub async fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.inner.recv_from(buf).await
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }
}

impl AsyncDatagramSocket for UdpSocket {
    fn send_to<'a>(
        &'a self,
        data: &'a [u8],
        target: SocketAddr,
    ) -> Pin<Box<dyn Future<Output = io::Result<usize>> + Send + 'a>> {
        Box::pin(async move { UdpSocket::send_to(self, data, target).await })
    }

    fn recv_from<'a>(
        &'a self,
        buf: &'a mut [u8],
    ) -> Pin<Box<dyn Future<Output = io::Result<(usize, SocketAddr)>> + Send + 'a>> {
        Box::pin(async move { UdpSocket::recv_from(self, buf).await })
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        UdpSocket::local_addr(self)
    }
}
