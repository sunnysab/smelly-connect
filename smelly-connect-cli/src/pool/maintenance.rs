use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::watch;
use tokio::task::JoinHandle;

pub(super) struct PoolMaintenance {
    shutdown_tx: watch::Sender<bool>,
    task: StdMutex<Option<JoinHandle<()>>>,
    pub(super) running: Arc<AtomicBool>,
}

impl PoolMaintenance {
    pub(super) fn new_shared() -> Arc<Self> {
        let (shutdown_tx, _shutdown_rx) = watch::channel(false);
        Arc::new(Self {
            shutdown_tx,
            task: StdMutex::new(None),
            running: Arc::new(AtomicBool::new(false)),
        })
    }

    pub(super) fn subscribe(&self) -> watch::Receiver<bool> {
        self.shutdown_tx.subscribe()
    }

    pub(super) fn install(&self, task: JoinHandle<()>) {
        let mut slot = self
            .task
            .lock()
            .expect("pool maintenance task mutex poisoned");
        if slot.is_some() {
            task.abort();
            return;
        }
        *slot = Some(task);
    }

    pub(super) fn signal_shutdown(&self) {
        self.shutdown_tx.send_replace(true);
    }

    pub(super) async fn shutdown(&self) {
        self.signal_shutdown();
        let task = {
            let mut slot = self
                .task
                .lock()
                .expect("pool maintenance task mutex poisoned");
            slot.take()
        };
        if let Some(task) = task {
            let _ = task.await;
        }
        self.running.store(false, Ordering::Release);
    }

    pub(super) fn abort(&self) {
        self.signal_shutdown();
        let task = {
            let mut slot = self
                .task
                .lock()
                .expect("pool maintenance task mutex poisoned");
            slot.take()
        };
        if let Some(task) = task {
            task.abort();
        }
        self.running.store(false, Ordering::Release);
    }
}
