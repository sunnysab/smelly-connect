use std::future::Future;
use std::io;
use std::net::Ipv4Addr;
use std::pin::Pin;
use std::sync::Arc;

use crate::TargetAddr;
use crate::transport::{VpnStream, VpnUdpSocket};

type ConnectFuture = Pin<Box<dyn Future<Output = io::Result<VpnStream>> + Send + 'static>>;
type BindUdpFuture = Pin<Box<dyn Future<Output = io::Result<VpnUdpSocket>> + Send + 'static>>;
type PingFuture = Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'static>>;
type Connector = dyn Fn(TargetAddr) -> ConnectFuture + Send + Sync + 'static;
type UdpBinder = dyn Fn() -> BindUdpFuture + Send + Sync + 'static;
type Pinger = dyn Fn(Ipv4Addr) -> PingFuture + Send + Sync + 'static;

#[derive(Clone)]
pub struct TransportStack {
    connector: Arc<Connector>,
    udp_binder: Option<Arc<UdpBinder>>,
    pinger: Option<Arc<Pinger>>,
}

impl TransportStack {
    pub fn new<F, Fut>(connector: F) -> Self
    where
        F: Fn(TargetAddr) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = io::Result<VpnStream>> + Send + 'static,
    {
        Self {
            connector: Arc::new(move |target| Box::pin(connector(target))),
            udp_binder: None,
            pinger: None,
        }
    }

    pub fn with_udp_binder<F, Fut>(mut self, binder: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = io::Result<VpnUdpSocket>> + Send + 'static,
    {
        self.udp_binder = Some(Arc::new(move || Box::pin(binder())));
        self
    }

    pub fn with_icmp_pinger<F, Fut>(mut self, pinger: F) -> Self
    where
        F: Fn(Ipv4Addr) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = io::Result<()>> + Send + 'static,
    {
        self.pinger = Some(Arc::new(move |target| Box::pin(pinger(target))));
        self
    }

    pub async fn connect<T>(&self, target: T) -> io::Result<VpnStream>
    where
        T: Into<TargetAddr>,
    {
        (self.connector)(target.into()).await
    }

    pub async fn bind_udp(&self) -> io::Result<VpnUdpSocket> {
        match &self.udp_binder {
            Some(binder) => binder().await,
            None => Err(io::Error::other("udp unsupported")),
        }
    }

    pub async fn icmp_ping(&self, target: Ipv4Addr) -> io::Result<()> {
        match &self.pinger {
            Some(pinger) => pinger(target).await,
            None => Err(io::Error::other("icmp ping unsupported")),
        }
    }
}
