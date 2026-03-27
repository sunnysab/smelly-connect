use std::collections::VecDeque;
use std::future::{pending, poll_fn};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{ChecksumCapabilities, Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::icmp::{
    self, PacketBuffer as IcmpPacketBuffer, PacketMetadata as IcmpPacketMetadata,
};
use smoltcp::socket::tcp::{self, SocketBuffer};
use smoltcp::socket::udp::{
    self, PacketBuffer as UdpPacketBuffer, PacketMetadata as UdpPacketMetadata,
};
use smoltcp::time::{Duration as SmolDuration, Instant};
use smoltcp::wire::{HardwareAddress, Icmpv4Packet, Icmpv4Repr, IpAddress, IpCidr, Ipv4Cidr};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{Notify, mpsc};

use crate::TargetAddr;
use crate::transport::datagram::AsyncDatagramSocket;
use crate::transport::device::PacketDevice;
use crate::transport::{TransportStack, VpnStream, VpnUdpSocket};

const TCP_BUFFER_SIZE: usize = 64 * 1024;
const TCP_KEEPALIVE_SECS: u64 = 30;
const TCP_TIMEOUT_SECS: u64 = 120;
const ICMP_BUFFER_SIZE: usize = 256;
const ICMP_KEEPALIVE_IDENT: u16 = 0x534d;
const ICMP_PING_TIMEOUT_MILLIS: u64 = 5_000;
const UDP_PACKET_CAPACITY: usize = 32;
const UDP_BUFFER_SIZE: usize = 64 * 1024;

#[derive(Clone)]
struct SmolStack {
    inner: Arc<SmolStackInner>,
}

struct SmolStackInner {
    state: Mutex<NetstackState>,
    wake: Notify,
    local_ip: Ipv4Addr,
}

struct NetstackState {
    device: QueueDevice,
    iface: Interface,
    sockets: SocketSet<'static>,
    active_handles: std::collections::HashSet<SocketHandle>,
    next_port: u16,
    next_icmp_seq: u16,
}

struct QueueDevice {
    caps: DeviceCapabilities,
    inbound: VecDeque<Vec<u8>>,
    outbound: VecDeque<Vec<u8>>,
}

struct QueueRxToken {
    packet: Vec<u8>,
}

struct QueueTxToken<'a> {
    queue: &'a mut VecDeque<Vec<u8>>,
}

struct SmolTcpStream {
    stack: Arc<SmolStackInner>,
    handle: SocketHandle,
}

struct PendingConnectGuard {
    stack: Arc<SmolStackInner>,
    handle: Option<SocketHandle>,
}

struct SmolUdpSocket {
    stack: Arc<SmolStackInner>,
    handle: SocketHandle,
    local_addr: SocketAddr,
}

pub fn build_transport_from_packet_device(
    mut device: PacketDevice,
    local_ip: Ipv4Addr,
) -> io::Result<TransportStack> {
    let inbound_rx = device
        .take_inbound_rx()
        .ok_or_else(|| io::Error::other("missing inbound rx"))?;
    let outbound_tx = device.outbound_sender();
    let stack = SmolStack::new(local_ip, inbound_rx, outbound_tx);
    let connect_stack = stack.clone();
    let udp_stack = stack.clone();
    let ping_stack = stack.clone();

    Ok(TransportStack::new(move |target: TargetAddr| {
        let stack = connect_stack.clone();
        async move {
            let addr = socket_addr_from_target(target)?;
            stack.connect(addr).await
        }
    })
    .with_udp_binder(move || {
        let stack = udp_stack.clone();
        async move { stack.bind_udp().await }
    })
    .with_icmp_pinger(move |target| {
        let stack = ping_stack.clone();
        async move { stack.ping(target).await }
    }))
}

impl SmolStack {
    fn new(
        local_ip: Ipv4Addr,
        inbound_rx: mpsc::Receiver<Vec<u8>>,
        outbound_tx: mpsc::Sender<Vec<u8>>,
    ) -> Self {
        let mut device = QueueDevice::new();
        let mut config = Config::new(HardwareAddress::Ip);
        config.random_seed = 1;

        let mut iface = Interface::new(config, &mut device, Instant::now());
        iface.update_ip_addrs(|ip_addrs| {
            ip_addrs
                .push(IpCidr::Ipv4(Ipv4Cidr::new(local_ip, 32)))
                .unwrap();
        });
        iface.routes_mut().add_default_ipv4_route(local_ip).unwrap();

        let inner = Arc::new(SmolStackInner {
            state: Mutex::new(NetstackState {
                device,
                iface,
                sockets: SocketSet::new(vec![]),
                active_handles: std::collections::HashSet::new(),
                next_port: 10000,
                next_icmp_seq: 1,
            }),
            wake: Notify::new(),
            local_ip,
        });

        let driver = Arc::clone(&inner);
        tokio::spawn(async move {
            let _ = driver.run(inbound_rx, outbound_tx).await;
        });

        Self { inner }
    }

    async fn connect(&self, addr: SocketAddr) -> io::Result<VpnStream> {
        if !matches!(addr.ip(), IpAddr::V4(_)) {
            return Err(io::Error::other("ipv6 unsupported"));
        }

        let handle = {
            let mut state = self.inner.state.lock().expect("netstack mutex poisoned");
            let socket = tcp_socket();
            let handle = state.sockets.add(socket);
            state.active_handles.insert(handle);
            let local_port = state.next_local_port();
            let remote_ip = match addr.ip() {
                IpAddr::V4(ip) => ip,
                IpAddr::V6(_) => return Err(io::Error::other("ipv6 unsupported")),
            };
            let NetstackState { iface, sockets, .. } = &mut *state;
            let cx = iface.context();
            sockets
                .get_mut::<tcp::Socket<'static>>(handle)
                .connect(cx, (remote_ip, addr.port()), local_port)
                .map_err(|err| io::Error::other(err.to_string()))?;
            handle
        };

        self.inner.wake.notify_one();
        let mut guard = PendingConnectGuard {
            stack: Arc::clone(&self.inner),
            handle: Some(handle),
        };

        let connected = poll_fn(|cx| self.inner.poll_connect(handle, cx)).await;
        connected?;
        guard.handle = None;

        Ok(VpnStream::new(SmolTcpStream {
            stack: Arc::clone(&self.inner),
            handle,
        }))
    }

    async fn ping(&self, target: Ipv4Addr) -> io::Result<()> {
        let (handle, seq_no) = {
            let mut state = self.inner.state.lock().expect("netstack mutex poisoned");
            let socket = icmp_socket();
            let handle = state.sockets.add(socket);
            state.active_handles.insert(handle);
            let seq_no = state.next_icmp_seq();
            let mut packet = [0_u8; 8];
            let repr = Icmpv4Repr::EchoRequest {
                ident: ICMP_KEEPALIVE_IDENT,
                seq_no,
                data: &[],
            };
            repr.emit(
                &mut Icmpv4Packet::new_unchecked(&mut packet),
                &ChecksumCapabilities::default(),
            );
            state
                .sockets
                .get_mut::<icmp::Socket<'static>>(handle)
                .bind(icmp::Endpoint::Ident(ICMP_KEEPALIVE_IDENT))
                .map_err(|err| io::Error::other(err.to_string()))?;
            state
                .sockets
                .get_mut::<icmp::Socket<'static>>(handle)
                .send_slice(&packet, IpAddress::Ipv4(target))
                .map_err(|err| io::Error::other(err.to_string()))?;
            (handle, seq_no)
        };

        self.inner.wake.notify_one();

        let deadline = tokio::time::Instant::now()
            + std::time::Duration::from_millis(ICMP_PING_TIMEOUT_MILLIS);
        let result = loop {
            let maybe_reply = {
                let mut state = self.inner.state.lock().expect("netstack mutex poisoned");
                let socket = state.sockets.get_mut::<icmp::Socket<'static>>(handle);
                if socket.can_recv() {
                    let mut buffer = [0_u8; ICMP_BUFFER_SIZE];
                    let (n, _) = socket
                        .recv_slice(&mut buffer)
                        .map_err(|err| io::Error::other(err.to_string()))?;
                    let packet = Icmpv4Packet::new_checked(&buffer[..n])
                        .map_err(|err| io::Error::other(err.to_string()))?;
                    match Icmpv4Repr::parse(&packet, &ChecksumCapabilities::default())
                        .map_err(|err| io::Error::other(err.to_string()))?
                    {
                        Icmpv4Repr::EchoReply { ident, seq_no, .. } => Some((ident, seq_no)),
                        _ => None,
                    }
                } else {
                    None
                }
            };

            if let Some((ident, reply_seq)) = maybe_reply
                && ident == ICMP_KEEPALIVE_IDENT
                && reply_seq == seq_no
            {
                break Ok(());
            }

            if tokio::time::Instant::now() >= deadline {
                break Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "icmp ping timed out",
                ));
            }

            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        };

        self.inner.remove_socket(handle);
        result
    }

    async fn bind_udp(&self) -> io::Result<VpnUdpSocket> {
        let (handle, local_addr) = {
            let mut state = self.inner.state.lock().expect("netstack mutex poisoned");
            let socket = udp_socket();
            let handle = state.sockets.add(socket);
            state.active_handles.insert(handle);
            let local_port = state.next_local_port();
            state
                .sockets
                .get_mut::<udp::Socket<'static>>(handle)
                .bind(local_port)
                .map_err(|err| io::Error::other(err.to_string()))?;
            (
                handle,
                SocketAddr::new(IpAddr::V4(self.inner.local_ip), local_port),
            )
        };
        self.inner.wake.notify_one();
        Ok(VpnUdpSocket::new(SmolUdpSocket {
            stack: Arc::clone(&self.inner),
            handle,
            local_addr,
        }))
    }
}

impl SmolStackInner {
    async fn run(
        self: Arc<Self>,
        mut inbound_rx: mpsc::Receiver<Vec<u8>>,
        outbound_tx: mpsc::Sender<Vec<u8>>,
    ) -> io::Result<()> {
        self.flush(None, &outbound_tx).await?;

        loop {
            let delay = self.next_delay();
            tokio::select! {
                maybe_packet = inbound_rx.recv() => {
                    let Some(packet) = maybe_packet else {
                        return Ok(());
                    };
                    self.flush(Some(packet), &outbound_tx).await?;
                }
                _ = self.wake.notified() => {
                    self.flush(None, &outbound_tx).await?;
                }
                _ = async {
                    match delay {
                        Some(delay) => tokio::time::sleep(delay).await,
                        None => pending::<()>().await,
                    }
                } => {
                    self.flush(None, &outbound_tx).await?;
                }
            }
        }
    }

    async fn flush(
        &self,
        inbound: Option<Vec<u8>>,
        outbound_tx: &mpsc::Sender<Vec<u8>>,
    ) -> io::Result<()> {
        let outbound_packets = {
            let mut state = self.state.lock().expect("netstack mutex poisoned");
            if let Some(packet) = inbound {
                state.device.push_inbound(packet);
            }
            state.poll();
            state.device.take_outbound()
        };

        for packet in outbound_packets {
            if outbound_tx.send(packet).await.is_err() {
                return Err(io::Error::other("packet transport closed"));
            }
        }
        Ok(())
    }

    fn next_delay(&self) -> Option<std::time::Duration> {
        let mut state = self.state.lock().expect("netstack mutex poisoned");
        let now = Instant::now();
        let NetstackState { iface, sockets, .. } = &mut *state;
        iface
            .poll_delay(now, sockets)
            .map(|delay| std::time::Duration::from_millis(delay.total_millis()))
    }

    fn poll_connect(&self, handle: SocketHandle, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut state = self.state.lock().expect("netstack mutex poisoned");
        let socket = state.sockets.get_mut::<tcp::Socket<'static>>(handle);

        if socket.may_send() {
            return Poll::Ready(Ok(()));
        }

        if matches!(socket.state(), tcp::State::Closed | tcp::State::TimeWait) {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "tcp connect failed",
            )));
        }

        socket.register_send_waker(cx.waker());
        Poll::Pending
    }

    fn remove_socket(&self, handle: SocketHandle) {
        let mut state = self.state.lock().expect("netstack mutex poisoned");
        if state.active_handles.remove(&handle) {
            let _ = state.sockets.remove(handle);
        }
    }
}

impl NetstackState {
    fn poll(&mut self) {
        let _ = self
            .iface
            .poll(Instant::now(), &mut self.device, &mut self.sockets);
    }

    fn next_local_port(&mut self) -> u16 {
        self.next_port = if self.next_port >= 60000 {
            10000
        } else {
            self.next_port + 1
        };
        self.next_port
    }

    fn next_icmp_seq(&mut self) -> u16 {
        let seq = self.next_icmp_seq;
        self.next_icmp_seq = self.next_icmp_seq.wrapping_add(1);
        seq
    }
}

impl QueueDevice {
    fn new() -> Self {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = 1500;
        caps.max_burst_size = Some(128);

        Self {
            caps,
            inbound: VecDeque::new(),
            outbound: VecDeque::new(),
        }
    }

    fn push_inbound(&mut self, packet: Vec<u8>) {
        self.inbound.push_back(packet);
    }

    fn take_outbound(&mut self) -> Vec<Vec<u8>> {
        self.outbound.drain(..).collect()
    }
}

impl Device for QueueDevice {
    type RxToken<'a>
        = QueueRxToken
    where
        Self: 'a;
    type TxToken<'a>
        = QueueTxToken<'a>
    where
        Self: 'a;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        self.inbound.pop_front().map(|packet| {
            (
                QueueRxToken { packet },
                QueueTxToken {
                    queue: &mut self.outbound,
                },
            )
        })
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(QueueTxToken {
            queue: &mut self.outbound,
        })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        self.caps.clone()
    }
}

impl RxToken for QueueRxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.packet)
    }
}

impl TxToken for QueueTxToken<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut packet = vec![0_u8; len];
        let result = f(&mut packet);
        self.queue.push_back(packet);
        result
    }
}

impl AsyncRead for SmolTcpStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }

        let mut state = self.stack.state.lock().expect("netstack mutex poisoned");
        let socket = state.sockets.get_mut::<tcp::Socket<'static>>(self.handle);

        if socket.can_recv() {
            let mut chunk = vec![0_u8; buf.remaining().min(16 * 1024)];
            match socket.recv_slice(&mut chunk) {
                Ok(n) => {
                    buf.put_slice(&chunk[..n]);
                    self.stack.wake.notify_one();
                    return Poll::Ready(Ok(()));
                }
                Err(err) => {
                    return Poll::Ready(Err(io::Error::other(err.to_string())));
                }
            }
        }

        if !socket.may_recv() {
            return Poll::Ready(Ok(()));
        }

        socket.register_recv_waker(cx.waker());
        Poll::Pending
    }
}

impl AsyncWrite for SmolTcpStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        let mut state = self.stack.state.lock().expect("netstack mutex poisoned");
        let socket = state.sockets.get_mut::<tcp::Socket<'static>>(self.handle);

        if socket.can_send() {
            let written = socket
                .send_slice(buf)
                .map_err(|err| io::Error::new(io::ErrorKind::BrokenPipe, err.to_string()))?;
            self.stack.wake.notify_one();
            return Poll::Ready(Ok(written));
        }

        if !socket.may_send() {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "tcp stream closed",
            )));
        }

        socket.register_send_waker(cx.waker());
        Poll::Pending
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        let mut state = self.stack.state.lock().expect("netstack mutex poisoned");
        let socket = state.sockets.get_mut::<tcp::Socket<'static>>(self.handle);

        if socket.send_queue() == 0 {
            return Poll::Ready(Ok(()));
        }

        socket.register_send_waker(cx.waker());
        Poll::Pending
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        let mut state = self.stack.state.lock().expect("netstack mutex poisoned");
        let socket = state.sockets.get_mut::<tcp::Socket<'static>>(self.handle);
        socket.close();
        self.stack.wake.notify_one();

        if socket.send_queue() == 0 {
            return Poll::Ready(Ok(()));
        }

        socket.register_send_waker(cx.waker());
        Poll::Pending
    }
}

impl Drop for SmolTcpStream {
    fn drop(&mut self) {
        self.stack.remove_socket(self.handle);
        self.stack.wake.notify_one();
    }
}

impl Drop for PendingConnectGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            self.stack.remove_socket(handle);
            self.stack.wake.notify_one();
        }
    }
}

impl AsyncDatagramSocket for SmolUdpSocket {
    fn send_to<'a>(
        &'a self,
        data: &'a [u8],
        target: SocketAddr,
    ) -> Pin<Box<dyn std::future::Future<Output = io::Result<usize>> + Send + 'a>> {
        Box::pin(async move {
            if !matches!(target.ip(), IpAddr::V4(_)) {
                return Err(io::Error::other("ipv6 unsupported"));
            }
            poll_fn(|cx| {
                let mut state = self.stack.state.lock().expect("netstack mutex poisoned");
                let socket = state.sockets.get_mut::<udp::Socket<'static>>(self.handle);
                if socket.can_send() {
                    let target = match target {
                        SocketAddr::V4(addr) => addr,
                        SocketAddr::V6(_) => {
                            return Poll::Ready(Err(io::Error::other("ipv6 unsupported")));
                        }
                    };
                    socket
                        .send_slice(data, target)
                        .map_err(|err| io::Error::other(err.to_string()))?;
                    self.stack.wake.notify_one();
                    return Poll::Ready(Ok(data.len()));
                }
                if !socket.is_open() {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "udp socket closed",
                    )));
                }
                socket.register_send_waker(cx.waker());
                Poll::Pending
            })
            .await
        })
    }

    fn recv_from<'a>(
        &'a self,
        buf: &'a mut [u8],
    ) -> Pin<Box<dyn std::future::Future<Output = io::Result<(usize, SocketAddr)>> + Send + 'a>>
    {
        Box::pin(async move {
            poll_fn(|cx| {
                let mut state = self.stack.state.lock().expect("netstack mutex poisoned");
                let socket = state.sockets.get_mut::<udp::Socket<'static>>(self.handle);
                if socket.can_recv() {
                    let (n, metadata) = socket
                        .recv_slice(buf)
                        .map_err(|err| io::Error::other(err.to_string()))?;
                    self.stack.wake.notify_one();
                    return Poll::Ready(
                        socket_addr_from_endpoint(metadata.endpoint).map(|addr| (n, addr)),
                    );
                }
                if !socket.is_open() {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "udp socket closed",
                    )));
                }
                socket.register_recv_waker(cx.waker());
                Poll::Pending
            })
            .await
        })
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.local_addr)
    }
}

impl Drop for SmolUdpSocket {
    fn drop(&mut self) {
        self.stack.remove_socket(self.handle);
        self.stack.wake.notify_one();
    }
}

fn socket_addr_from_target(target: TargetAddr) -> io::Result<SocketAddr> {
    let ip = target
        .host()
        .parse::<IpAddr>()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "expected resolved IP target"))?;
    match ip {
        IpAddr::V4(ip) => Ok(SocketAddr::new(IpAddr::V4(ip), target.port())),
        IpAddr::V6(_) => Err(io::Error::other("ipv6 unsupported")),
    }
}

fn tcp_socket() -> tcp::Socket<'static> {
    let rx = SocketBuffer::new(vec![0; TCP_BUFFER_SIZE]);
    let tx = SocketBuffer::new(vec![0; TCP_BUFFER_SIZE]);
    let mut socket = tcp::Socket::new(rx, tx);
    socket.set_keep_alive(Some(SmolDuration::from_secs(TCP_KEEPALIVE_SECS)));
    socket.set_timeout(Some(SmolDuration::from_secs(TCP_TIMEOUT_SECS)));
    socket
}

fn icmp_socket() -> icmp::Socket<'static> {
    let rx = IcmpPacketBuffer::new(vec![IcmpPacketMetadata::EMPTY], vec![0; ICMP_BUFFER_SIZE]);
    let tx = IcmpPacketBuffer::new(vec![IcmpPacketMetadata::EMPTY], vec![0; ICMP_BUFFER_SIZE]);
    icmp::Socket::new(rx, tx)
}

fn udp_socket() -> udp::Socket<'static> {
    let rx = UdpPacketBuffer::new(
        vec![UdpPacketMetadata::EMPTY; UDP_PACKET_CAPACITY],
        vec![0; UDP_BUFFER_SIZE],
    );
    let tx = UdpPacketBuffer::new(
        vec![UdpPacketMetadata::EMPTY; UDP_PACKET_CAPACITY],
        vec![0; UDP_BUFFER_SIZE],
    );
    udp::Socket::new(rx, tx)
}

fn socket_addr_from_endpoint(endpoint: smoltcp::wire::IpEndpoint) -> io::Result<SocketAddr> {
    match endpoint.addr {
        IpAddress::Ipv4(ip) => Ok(SocketAddr::new(IpAddr::V4(ip), endpoint.port)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancelled_connect_releases_pending_socket_handle() {
        let (_vpn_tx, vpn_rx) = mpsc::channel(4);
        let (stack_tx, _stack_rx) = mpsc::channel(4);
        let stack = SmolStack::new(Ipv4Addr::new(10, 0, 0, 8), vpn_rx, stack_tx);

        let result = tokio::time::timeout(
            std::time::Duration::from_millis(20),
            stack.connect(SocketAddr::from((Ipv4Addr::new(10, 0, 0, 9), 443))),
        )
        .await;
        assert!(result.is_err(), "connect should time out in test");

        let state = stack.inner.state.lock().expect("netstack mutex poisoned");
        assert!(
            state.active_handles.is_empty(),
            "timed out connect leaked active socket handles"
        );
    }
}
