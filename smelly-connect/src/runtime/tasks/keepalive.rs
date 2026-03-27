use crate::error::{Error, TransportError};

pub struct KeepaliveHandle {
    pub(crate) shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    pub(crate) task: Option<tokio::task::JoinHandle<()>>,
}

impl KeepaliveHandle {
    pub async fn shutdown(mut self) -> Result<(), Error> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.task.take() {
            task.await
                .map_err(|err| Error::Transport(TransportError::ConnectFailed(err.to_string())))?;
        }
        Ok(())
    }
}

impl Drop for KeepaliveHandle {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
