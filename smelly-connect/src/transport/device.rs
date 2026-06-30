use tokio::sync::mpsc;

/// Runtime handle for a packet device.
///
/// This holds only the sender channels — the receivers are consumed during
/// construction (see `build_transport_from_packet_device`).
#[derive(Clone)]
pub struct PacketDevice {
    inbound_tx: mpsc::Sender<Vec<u8>>,
    outbound_tx: mpsc::Sender<Vec<u8>>,
}

impl PacketDevice {
    pub fn new(inbound_tx: mpsc::Sender<Vec<u8>>, outbound_tx: mpsc::Sender<Vec<u8>>) -> Self {
        Self {
            inbound_tx,
            outbound_tx,
        }
    }

    /// Inject a packet from the VPN tunnel into the smoltcp stack.
    pub async fn inject_from_vpn(&self, packet: Vec<u8>) {
        let _ = self.inbound_tx.send(packet).await;
    }

    pub fn outbound_sender(&self) -> mpsc::Sender<Vec<u8>> {
        self.outbound_tx.clone()
    }
}
