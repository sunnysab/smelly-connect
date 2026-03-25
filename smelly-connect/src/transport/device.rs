use tokio::sync::{Mutex, mpsc};

pub struct PacketDevice {
    inbound_tx: mpsc::Sender<Vec<u8>>,
    inbound_rx: Option<Mutex<mpsc::Receiver<Vec<u8>>>>,
    outbound_tx: mpsc::Sender<Vec<u8>>,
    outbound_rx: Option<Mutex<mpsc::Receiver<Vec<u8>>>>,
}

impl PacketDevice {
    pub fn new(
        inbound_tx: mpsc::Sender<Vec<u8>>,
        inbound_rx: mpsc::Receiver<Vec<u8>>,
        outbound_tx: mpsc::Sender<Vec<u8>>,
        outbound_rx: mpsc::Receiver<Vec<u8>>,
    ) -> Self {
        Self {
            inbound_tx,
            inbound_rx: Some(Mutex::new(inbound_rx)),
            outbound_tx,
            outbound_rx: Some(Mutex::new(outbound_rx)),
        }
    }

    pub async fn inject_from_vpn(&self, packet: Vec<u8>) {
        let _ = self.inbound_tx.send(packet).await;
    }

    pub async fn read_for_stack(&self) -> Option<Vec<u8>> {
        let inbound_rx = self.inbound_rx.as_ref()?;
        inbound_rx.lock().await.recv().await
    }

    pub async fn write_from_stack(&self, packet: Vec<u8>) {
        let _ = self.outbound_tx.send(packet).await;
    }

    pub async fn read_for_vpn(&self) -> Option<Vec<u8>> {
        let outbound_rx = self.outbound_rx.as_ref()?;
        outbound_rx.lock().await.recv().await
    }

    pub fn take_outbound_rx(&mut self) -> Option<mpsc::Receiver<Vec<u8>>> {
        self.outbound_rx.take().map(|mutex| mutex.into_inner())
    }

    pub fn take_inbound_rx(&mut self) -> Option<mpsc::Receiver<Vec<u8>>> {
        self.inbound_rx.take().map(|mutex| mutex.into_inner())
    }

    pub fn outbound_sender(&self) -> mpsc::Sender<Vec<u8>> {
        self.outbound_tx.clone()
    }
}
