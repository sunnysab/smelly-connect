use std::io;
use std::net::{Ipv4Addr, SocketAddr};

use tokio::io::duplex;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

use crate::transport::device::PacketDevice;
use crate::transport::stack::TransportStack;

/// Test harness that provides both directions of packet flow without needing a
/// real VPN tunnel.  Unlike the production `PacketDevice` (which only holds
/// senders), this harness also keeps the channel receivers so tests can assert
/// on what flows through.
pub struct PacketHarness {
    device: PacketDevice,
    /// Sender for outbound direction (stack → VPN).
    outbound_tx: mpsc::Sender<Vec<u8>>,
    /// Receiver for outbound packets (stack → VPN).
    outbound_rx: mpsc::Receiver<Vec<u8>>,
    /// Receiver for inbound packets (VPN → stack).
    inbound_rx: mpsc::Receiver<Vec<u8>>,
}

impl PacketHarness {
    /// Inject a packet as if from the VPN tunnel (arrives at the stack side).
    pub async fn inject_from_vpn(&self, packet: Vec<u8>) {
        self.device.inject_from_vpn(packet).await;
    }

    /// Read a packet that arrived at the stack (previously injected via
    /// `inject_from_vpn`).
    pub async fn read_for_stack(&mut self) -> Option<Vec<u8>> {
        self.inbound_rx.recv().await
    }

    /// Write a packet as if from the stack (arrives at the VPN side).
    pub async fn write_from_stack(&self, packet: Vec<u8>) {
        let _ = self.outbound_tx.send(packet).await;
    }

    /// Read a packet sent by the stack (previously written via
    /// `write_from_stack`).
    pub async fn read_for_vpn(&mut self) -> Option<Vec<u8>> {
        self.outbound_rx.recv().await
    }

    /// Consume the harness and return the underlying device.
    pub fn into_device(self) -> PacketDevice {
        self.device
    }
}

pub fn packet_harness() -> PacketHarness {
    let (vpn_tx, vpn_rx) = mpsc::channel(4);
    let (stack_tx, stack_rx) = mpsc::channel(4);
    let device = PacketDevice::new(vpn_tx, stack_tx.clone());
    PacketHarness {
        device,
        outbound_tx: stack_tx,
        outbound_rx: stack_rx,
        inbound_rx: vpn_rx,
    }
}

pub struct StackHarness {
    stack: TransportStack,
}

impl StackHarness {
    pub async fn connect<T>(&self, target: T) -> io::Result<crate::transport::VpnStream>
    where
        T: Into<crate::TargetAddr>,
    {
        self.stack.connect(target).await
    }

    pub async fn bind_udp(&self) -> io::Result<crate::transport::VpnUdpSocket> {
        self.stack.bind_udp().await
    }
}

pub fn stack_harness() -> StackHarness {
    let stack = TransportStack::new(|_| async {
        let (client, _server) = duplex(1024);
        Ok(crate::transport::VpnStream::new(client))
    })
    .with_udp_binder(|| async {
        let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
        Ok(crate::transport::VpnUdpSocket::new(socket))
    });
    StackHarness { stack }
}
