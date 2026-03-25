use crate::error::{Error, TransportError};

pub struct KeepaliveHandle {
    pub(crate) shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    pub(crate) task: tokio::task::JoinHandle<()>,
}

impl KeepaliveHandle {
    pub async fn shutdown(mut self) -> Result<(), Error> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        self.task.await.map_err(|err| {
            Error::Transport(TransportError::ConnectFailed(err.to_string()))
        })?;
        Ok(())
    }
}
