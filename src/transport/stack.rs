use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;

use crate::TargetAddr;
use crate::transport::stream::VpnStream;

type ConnectFuture = Pin<Box<dyn Future<Output = io::Result<VpnStream>> + Send + 'static>>;
type Connector = dyn Fn(TargetAddr) -> ConnectFuture + Send + Sync + 'static;

#[derive(Clone)]
pub struct TransportStack {
    connector: Arc<Connector>,
}

impl TransportStack {
    pub fn new<F, Fut>(connector: F) -> Self
    where
        F: Fn(TargetAddr) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = io::Result<VpnStream>> + Send + 'static,
    {
        Self {
            connector: Arc::new(move |target| Box::pin(connector(target))),
        }
    }

    pub async fn connect<T>(&self, target: T) -> io::Result<VpnStream>
    where
        T: Into<TargetAddr>,
    {
        (self.connector)(target.into()).await
    }
}
