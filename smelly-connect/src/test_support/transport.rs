use std::io;
use std::net::{Ipv4Addr, SocketAddr};

use tokio::io::duplex;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

use crate::transport::device::PacketDevice;
use crate::transport::stack::TransportStack;

pub struct PacketHarness {
    device: PacketDevice,
}

impl PacketHarness {
    pub async fn inject_from_vpn(&self, packet: Vec<u8>) {
        self.device.inject_from_vpn(packet).await;
    }

    pub async fn read_for_stack(&self) -> Vec<u8> {
        self.device.read_for_stack().await.unwrap()
    }

    pub async fn write_from_stack(&self, packet: Vec<u8>) {
        self.device.write_from_stack(packet).await;
    }

    pub async fn read_for_vpn(&self) -> Vec<u8> {
        self.device.read_for_vpn().await.unwrap()
    }

    pub fn into_device(self) -> PacketDevice {
        self.device
    }
}

pub fn packet_harness() -> PacketHarness {
    let (vpn_tx, vpn_rx) = mpsc::channel(4);
    let (stack_tx, stack_rx) = mpsc::channel(4);
    let device = PacketDevice::new(vpn_tx, vpn_rx, stack_tx, stack_rx);
    PacketHarness { device }
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
