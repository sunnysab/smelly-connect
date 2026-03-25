use std::io;
use std::sync::Arc;

use crate::TargetAddr;
use crate::transport::stream::VpnStream;

type Connector = dyn Fn(TargetAddr) -> io::Result<VpnStream> + Send + Sync + 'static;

#[derive(Clone)]
pub struct TransportStack {
    connector: Arc<Connector>,
}

impl TransportStack {
    pub fn new<F>(connector: F) -> Self
    where
        F: Fn(TargetAddr) -> io::Result<VpnStream> + Send + Sync + 'static,
    {
        Self {
            connector: Arc::new(connector),
        }
    }

    pub async fn connect<T>(&self, target: T) -> io::Result<VpnStream>
    where
        T: Into<TargetAddr>,
    {
        (self.connector)(target.into())
    }
}
