use tokio::sync::{Mutex, mpsc};

pub struct PacketDevice {
    inbound_tx: mpsc::Sender<Vec<u8>>,
    inbound_rx: Mutex<mpsc::Receiver<Vec<u8>>>,
    #[allow(dead_code)]
    outbound_tx: mpsc::Sender<Vec<u8>>,
}

impl PacketDevice {
    pub fn new(
        inbound_tx: mpsc::Sender<Vec<u8>>,
        inbound_rx: mpsc::Receiver<Vec<u8>>,
        outbound_tx: mpsc::Sender<Vec<u8>>,
    ) -> Self {
        Self {
            inbound_tx,
            inbound_rx: Mutex::new(inbound_rx),
            outbound_tx,
        }
    }

    pub async fn inject_from_vpn(&self, packet: Vec<u8>) {
        let _ = self.inbound_tx.send(packet).await;
    }

    pub async fn read_for_stack(&self) -> Option<Vec<u8>> {
        self.inbound_rx.lock().await.recv().await
    }
}
