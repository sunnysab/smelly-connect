use std::collections::{HashMap, HashSet, VecDeque};
use std::future::{pending, poll_fn};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};

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
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant as TokioInstant;
use tracing::{debug, warn};

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
const SOCKET_CHUNK_SIZE: usize = 16 * 1024;

#[derive(Clone)]
struct SmolStack {
    actor: NetstackActorHandle,
}

#[derive(Clone)]
struct NetstackActorHandle {
    commands: mpsc::UnboundedSender<NetstackCommand>,
}

struct NetstackActor {
    device: QueueDevice,
    iface: Interface,
    sockets: SocketSet<'static>,
    active_handles: HashSet<SocketHandle>,
    tcp_sockets: HashMap<SocketHandle, Arc<TcpSocketState>>,
    udp_sockets: HashMap<SocketHandle, Arc<UdpSocketState>>,
    pending_pings: HashMap<SocketHandle, PendingPing>,
    pending_outbound: VecDeque<Vec<u8>>,
    local_ip: Ipv4Addr,
    next_port: u16,
    next_icmp_seq: u16,
}

enum NetstackCommand {
    TcpConnect {
        addr: SocketAddr,
        state: Arc<TcpSocketState>,
        reply: oneshot::Sender<io::Result<SocketHandle>>,
    },
    UdpBind {
        state: Arc<UdpSocketState>,
        reply: oneshot::Sender<io::Result<(SocketHandle, SocketAddr)>>,
    },
    Ping {
        target: Ipv4Addr,
        reply: oneshot::Sender<io::Result<()>>,
    },
    SocketRead(SocketHandle),
    SocketWrite(SocketHandle),
    SocketFlush(SocketHandle),
    TcpClose(SocketHandle),
    RemoveSocket(SocketHandle),
    UdpSend {
        handle: SocketHandle,
        target: SocketAddrV4,
        data: Vec<u8>,
        reply: oneshot::Sender<io::Result<usize>>,
    },
    #[cfg(test)]
    TestQueueOutbound {
        packet: Vec<u8>,
    },
    #[cfg(test)]
    TestSnapshot {
        reply: oneshot::Sender<NetstackSnapshot>,
    },
}

struct PendingPing {
    seq_no: u16,
    reply: oneshot::Sender<io::Result<()>>,
    deadline: TokioInstant,
}

#[derive(Clone, Debug)]
struct SharedIoError {
    kind: io::ErrorKind,
    message: Arc<str>,
}

struct TcpSocketState {
    shared: Mutex<TcpSocketShared>,
}

struct TcpSocketShared {
    connect_result: Option<Result<(), SharedIoError>>,
    read_buffer: VecDeque<u8>,
    write_buffer: VecDeque<u8>,
    read_closed: bool,
    send_open: bool,
    send_queue_empty: bool,
    close_requested: bool,
    close_sent: bool,
    terminal_error: Option<SharedIoError>,
    connect_waker: Option<Waker>,
    read_waker: Option<Waker>,
    write_waker: Option<Waker>,
    flush_waker: Option<Waker>,
    shutdown_waker: Option<Waker>,
}

struct UdpSocketState {
    shared: Mutex<UdpSocketShared>,
}

struct UdpSocketShared {
    recv_queue: VecDeque<(Vec<u8>, SocketAddr)>,
    pending_sends: VecDeque<PendingUdpSend>,
    closed: bool,
    error: Option<SharedIoError>,
    recv_waker: Option<Waker>,
}

struct PendingUdpSend {
    data: Vec<u8>,
    target: SocketAddrV4,
    reply: oneshot::Sender<io::Result<usize>>,
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
    actor: NetstackActorHandle,
    state: Arc<TcpSocketState>,
    handle: SocketHandle,
}

struct PendingConnectGuard {
    actor: NetstackActorHandle,
    handle: Option<SocketHandle>,
}

struct SmolUdpSocket {
    actor: NetstackActorHandle,
    state: Arc<UdpSocketState>,
    handle: SocketHandle,
    local_addr: SocketAddr,
}

#[cfg(test)]
struct NetstackSnapshot {
    active_handles: usize,
    pending_outbound: usize,
}

/// Acquire the mutex lock. If the lock is poisoned, recover it
/// so a single task panic does not crash the whole process.
fn acquire_lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            warn!("netstack mutex recovered from poison");
            poisoned.into_inner()
        }
    }
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

    // Move the device into the connect closure so it stays alive as long as
    // the TransportStack exists. Without this, the device's inbound_tx sender
    // would be dropped, causing the driver loop to exit immediately.
    Ok(TransportStack::new(move |target: TargetAddr| {
        let stack = connect_stack.clone();
        let _device = &device; // keep device alive
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
        iface
            .routes_mut()
            .add_default_ipv4_route(Ipv4Addr::UNSPECIFIED)
            .unwrap();

        let (commands_tx, commands_rx) = mpsc::unbounded_channel();
        let actor = NetstackActor {
            device,
            iface,
            sockets: SocketSet::new(vec![]),
            active_handles: HashSet::new(),
            tcp_sockets: HashMap::new(),
            udp_sockets: HashMap::new(),
            pending_pings: HashMap::new(),
            pending_outbound: VecDeque::new(),
            local_ip,
            next_port: 10000,
            next_icmp_seq: 1,
        };

        tokio::spawn(async move {
            if let Err(err) = actor.run(inbound_rx, outbound_tx, commands_rx).await {
                warn!(error = %err, "netstack actor stopped");
            }
        });

        Self {
            actor: NetstackActorHandle {
                commands: commands_tx,
            },
        }
    }

    async fn connect(&self, addr: SocketAddr) -> io::Result<VpnStream> {
        if !matches!(addr.ip(), IpAddr::V4(_)) {
            return Err(io::Error::other("ipv6 unsupported"));
        }

        let state = Arc::new(TcpSocketState::new());
        let (reply_tx, reply_rx) = oneshot::channel();
        self.actor.send(NetstackCommand::TcpConnect {
            addr,
            state: Arc::clone(&state),
            reply: reply_tx,
        })?;
        let handle = recv_actor_reply(reply_rx).await?;

        let mut guard = PendingConnectGuard {
            actor: self.actor.clone(),
            handle: Some(handle),
        };
        poll_fn(|cx| state.poll_connect(cx)).await?;
        guard.handle = None;

        Ok(VpnStream::new(SmolTcpStream {
            actor: self.actor.clone(),
            state,
            handle,
        }))
    }

    async fn ping(&self, target: Ipv4Addr) -> io::Result<()> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.actor.send(NetstackCommand::Ping {
            target,
            reply: reply_tx,
        })?;
        recv_actor_reply(reply_rx).await
    }

    async fn bind_udp(&self) -> io::Result<VpnUdpSocket> {
        let state = Arc::new(UdpSocketState::new());
        let (reply_tx, reply_rx) = oneshot::channel();
        self.actor.send(NetstackCommand::UdpBind {
            state: Arc::clone(&state),
            reply: reply_tx,
        })?;
        let (handle, local_addr) = recv_actor_reply(reply_rx).await?;

        Ok(VpnUdpSocket::new(SmolUdpSocket {
            actor: self.actor.clone(),
            state,
            handle,
            local_addr,
        }))
    }

    #[cfg(test)]
    async fn queue_outbound_for_test(&self, packet: Vec<u8>) {
        self.actor
            .send(NetstackCommand::TestQueueOutbound { packet })
            .expect("test command should reach netstack actor");
    }

    #[cfg(test)]
    async fn snapshot(&self) -> NetstackSnapshot {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.actor
            .send(NetstackCommand::TestSnapshot { reply: reply_tx })
            .expect("test snapshot should reach netstack actor");
        reply_rx.await.expect("test snapshot reply should succeed")
    }
}

impl NetstackActorHandle {
    fn send(&self, command: NetstackCommand) -> io::Result<()> {
        self.commands
            .send(command)
            .map_err(|_| io::Error::other("netstack actor stopped"))
    }

    fn remove_socket(&self, handle: SocketHandle) {
        let _ = self.commands.send(NetstackCommand::RemoveSocket(handle));
    }

    fn notify_read(&self, handle: SocketHandle) {
        let _ = self.commands.send(NetstackCommand::SocketRead(handle));
    }

    fn notify_write(&self, handle: SocketHandle) {
        let _ = self.commands.send(NetstackCommand::SocketWrite(handle));
    }

    fn notify_flush(&self, handle: SocketHandle) {
        let _ = self.commands.send(NetstackCommand::SocketFlush(handle));
    }

    fn request_close(&self, handle: SocketHandle) {
        let _ = self.commands.send(NetstackCommand::TcpClose(handle));
    }
}

impl NetstackActor {
    async fn run(
        mut self,
        mut inbound_rx: mpsc::Receiver<Vec<u8>>,
        outbound_tx: mpsc::Sender<Vec<u8>>,
        mut commands_rx: mpsc::UnboundedReceiver<NetstackCommand>,
    ) -> io::Result<()> {
        let mut commands_open = true;

        loop {
            self.drive();
            if let Err(err) = self.flush_outbound(&outbound_tx) {
                self.fail_all(SharedIoError::new(err.kind(), err.to_string()));
                return Err(err);
            }

            let delay = self.next_delay();
            let shutdown_error = tokio::select! {
                biased;
                maybe_command = async {
                    if commands_open {
                        commands_rx.recv().await
                    } else {
                        pending::<Option<NetstackCommand>>().await
                    }
                } => {
                    match maybe_command {
                        Some(command) => {
                            self.handle_command(command)?;
                            None
                        }
                        None => {
                            commands_open = false;
                            None
                        }
                    }
                }
                maybe_packet = inbound_rx.recv() => {
                    match maybe_packet {
                        Some(packet) => {
                            self.device.push_inbound(packet);
                            None
                        }
                        None => Some(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "packet source closed",
                        )),
                    }
                }
                _ = async {
                    match delay {
                        Some(delay) => tokio::time::sleep(delay).await,
                        None => pending::<()>().await,
                    }
                } => None
            };

            if let Some(err) = shutdown_error {
                self.fail_all(SharedIoError::new(err.kind(), err.to_string()));
                return Err(err);
            }

            if !commands_open && inbound_rx.is_closed() {
                return Ok(());
            }
        }
    }

    fn handle_command(&mut self, command: NetstackCommand) -> io::Result<()> {
        match command {
            NetstackCommand::TcpConnect { addr, state, reply } => {
                let _ = reply.send(self.start_tcp_connect(addr, state));
            }
            NetstackCommand::UdpBind { state, reply } => {
                let _ = reply.send(self.start_udp_bind(state));
            }
            NetstackCommand::Ping { target, reply } => {
                if let Err(err) = self.start_ping(target, reply) {
                    warn!(error = %err, "netstack ping setup failed");
                }
            }
            NetstackCommand::SocketRead(handle)
            | NetstackCommand::SocketWrite(handle)
            | NetstackCommand::SocketFlush(handle)
            | NetstackCommand::TcpClose(handle) => {
                debug!(?handle, "netstack actor socket wake");
            }
            NetstackCommand::RemoveSocket(handle) => {
                self.remove_socket(handle);
            }
            NetstackCommand::UdpSend {
                handle,
                target,
                data,
                reply,
            } => {
                if let Some(state) = self.udp_sockets.get(&handle).cloned() {
                    state.enqueue_send(target, data, reply);
                } else {
                    let _ = reply.send(Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "udp socket closed",
                    )));
                }
            }
            #[cfg(test)]
            NetstackCommand::TestQueueOutbound { packet } => {
                self.device.outbound.push_back(packet);
            }
            #[cfg(test)]
            NetstackCommand::TestSnapshot { reply } => {
                let _ = reply.send(NetstackSnapshot {
                    active_handles: self.active_handles.len(),
                    pending_outbound: self.pending_outbound.len(),
                });
            }
        }

        Ok(())
    }

    fn start_tcp_connect(
        &mut self,
        addr: SocketAddr,
        state: Arc<TcpSocketState>,
    ) -> io::Result<SocketHandle> {
        let remote_ip = match addr.ip() {
            IpAddr::V4(ip) => ip,
            IpAddr::V6(_) => return Err(io::Error::other("ipv6 unsupported")),
        };

        let handle = self.sockets.add(tcp_socket());
        self.active_handles.insert(handle);
        self.tcp_sockets.insert(handle, Arc::clone(&state));

        let local_port = self.next_local_port();
        let connect_result = {
            let (iface, sockets) = (&mut self.iface, &mut self.sockets);
            sockets.get_mut::<tcp::Socket<'static>>(handle).connect(
                iface.context(),
                (remote_ip, addr.port()),
                local_port,
            )
        };

        if let Err(err) = connect_result {
            self.remove_socket(handle);
            return Err(io::Error::other(err.to_string()));
        }

        debug!(
            ?handle,
            local_port,
            remote_addr = %addr,
            active_handles = self.active_handles.len(),
            pending_outbound = self.pending_outbound.len(),
            "netstack tcp connect started"
        );

        Ok(handle)
    }

    fn start_udp_bind(
        &mut self,
        state: Arc<UdpSocketState>,
    ) -> io::Result<(SocketHandle, SocketAddr)> {
        let handle = self.sockets.add(udp_socket());
        self.active_handles.insert(handle);
        self.udp_sockets.insert(handle, Arc::clone(&state));

        let local_port = self.next_local_port();
        if let Err(err) = self
            .sockets
            .get_mut::<udp::Socket<'static>>(handle)
            .bind(local_port)
        {
            self.remove_socket(handle);
            return Err(io::Error::other(err.to_string()));
        }

        Ok((
            handle,
            SocketAddr::new(IpAddr::V4(self.local_ip), local_port),
        ))
    }

    fn start_ping(
        &mut self,
        target: Ipv4Addr,
        reply: oneshot::Sender<io::Result<()>>,
    ) -> io::Result<()> {
        let handle = self.sockets.add(icmp_socket());
        self.active_handles.insert(handle);
        let seq_no = self.next_icmp_seq();

        let result = (|| {
            let socket = self.sockets.get_mut::<icmp::Socket<'static>>(handle);
            socket
                .bind(icmp::Endpoint::Ident(ICMP_KEEPALIVE_IDENT))
                .map_err(|err| io::Error::other(err.to_string()))?;

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

            socket
                .send_slice(&packet, IpAddress::Ipv4(target))
                .map_err(|err| io::Error::other(err.to_string()))
        })();

        if let Err(err) = result {
            self.remove_socket(handle);
            let _ = reply.send(Err(err));
            return Ok(());
        }

        self.pending_pings.insert(
            handle,
            PendingPing {
                seq_no,
                reply,
                deadline: TokioInstant::now()
                    + std::time::Duration::from_millis(ICMP_PING_TIMEOUT_MILLIS),
            },
        );

        Ok(())
    }

    fn drive(&mut self) {
        for _ in 0..16 {
            let outbound_before = self.device.outbound.len();
            let mut progressed = false;

            progressed |= self.sync_tcp_sockets();
            progressed |= self.sync_udp_sockets();
            let _ = self
                .iface
                .poll(Instant::now(), &mut self.device, &mut self.sockets);
            progressed |= self.sync_tcp_sockets();
            progressed |= self.sync_udp_sockets();
            progressed |= self.sync_pending_pings();

            if !progressed && self.device.outbound.len() == outbound_before {
                break;
            }
        }

        self.pending_outbound.extend(self.device.take_outbound());
    }

    fn sync_tcp_sockets(&mut self) -> bool {
        let handles: Vec<_> = self.tcp_sockets.keys().copied().collect();
        let mut progressed = false;

        for handle in handles {
            let Some(state) = self.tcp_sockets.get(&handle).cloned() else {
                continue;
            };
            let socket = self.sockets.get_mut::<tcp::Socket<'static>>(handle);
            progressed |= sync_tcp_socket(socket, &state);
        }

        progressed
    }

    fn sync_udp_sockets(&mut self) -> bool {
        let handles: Vec<_> = self.udp_sockets.keys().copied().collect();
        let mut progressed = false;

        for handle in handles {
            let Some(state) = self.udp_sockets.get(&handle).cloned() else {
                continue;
            };
            let socket = self.sockets.get_mut::<udp::Socket<'static>>(handle);
            progressed |= sync_udp_socket(socket, &state);
        }

        progressed
    }

    fn sync_pending_pings(&mut self) -> bool {
        let handles: Vec<_> = self.pending_pings.keys().copied().collect();
        let mut progressed = false;
        let now = TokioInstant::now();
        let mut done = Vec::new();

        for handle in handles {
            let Some(ping) = self.pending_pings.get(&handle) else {
                continue;
            };

            if now >= ping.deadline {
                done.push((
                    handle,
                    Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "icmp ping timed out",
                    )),
                ));
                progressed = true;
                continue;
            }

            let socket = self.sockets.get_mut::<icmp::Socket<'static>>(handle);
            if !socket.can_recv() {
                continue;
            }

            let mut buffer = [0_u8; ICMP_BUFFER_SIZE];
            let result = socket
                .recv_slice(&mut buffer)
                .map_err(|err| io::Error::other(err.to_string()))
                .and_then(|(n, _)| {
                    let packet = Icmpv4Packet::new_checked(&buffer[..n])
                        .map_err(|err| io::Error::other(err.to_string()))?;
                    let repr = Icmpv4Repr::parse(&packet, &ChecksumCapabilities::default())
                        .map_err(|err| io::Error::other(err.to_string()))?;
                    match repr {
                        Icmpv4Repr::EchoReply { ident, seq_no, .. }
                            if ident == ICMP_KEEPALIVE_IDENT && seq_no == ping.seq_no =>
                        {
                            Ok(())
                        }
                        _ => Err(io::Error::other("unexpected icmp echo response")),
                    }
                });

            done.push((handle, result));
            progressed = true;
        }

        for (handle, result) in done {
            if let Some(ping) = self.pending_pings.remove(&handle) {
                let _ = ping.reply.send(result);
            }
            self.remove_socket(handle);
        }

        progressed
    }

    fn next_delay(&mut self) -> Option<std::time::Duration> {
        if !self.pending_outbound.is_empty() || self.has_pending_local_work() {
            return Some(std::time::Duration::from_millis(1));
        }

        let mut delay = self
            .iface
            .poll_delay(Instant::now(), &self.sockets)
            .map(|delay| std::time::Duration::from_millis(delay.total_millis()));

        let now = TokioInstant::now();
        for ping in self.pending_pings.values() {
            let ping_delay = ping.deadline.saturating_duration_since(now);
            delay = Some(match delay {
                Some(current) => current.min(ping_delay),
                None => ping_delay,
            });
        }

        delay
    }

    fn has_pending_local_work(&self) -> bool {
        self.tcp_sockets.values().any(|state| {
            let shared = acquire_lock(&state.shared);
            !shared.write_buffer.is_empty()
                || (shared.close_requested && (!shared.close_sent || !shared.send_queue_empty))
        }) || self.udp_sockets.values().any(|state| {
            let shared = acquire_lock(&state.shared);
            !shared.pending_sends.is_empty()
        })
    }

    fn flush_outbound(&mut self, outbound_tx: &mpsc::Sender<Vec<u8>>) -> io::Result<()> {
        while let Some(packet) = self.pending_outbound.pop_front() {
            match outbound_tx.try_send(packet) {
                Ok(()) => {}
                Err(TrySendError::Full(packet)) => {
                    self.pending_outbound.push_front(packet);
                    warn!(
                        pending_outbound = self.pending_outbound.len(),
                        active_handles = self.active_handles.len(),
                        "netstack outbound queue backed up"
                    );
                    break;
                }
                Err(TrySendError::Closed(_packet)) => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "packet transport closed",
                    ));
                }
            }
        }

        Ok(())
    }

    fn remove_socket(&mut self, handle: SocketHandle) {
        if !self.active_handles.remove(&handle) {
            return;
        }

        if let Some(state) = self.tcp_sockets.remove(&handle) {
            state.on_removed(Some(SharedIoError::new(
                io::ErrorKind::ConnectionAborted,
                "tcp socket removed",
            )));
        }

        if let Some(state) = self.udp_sockets.remove(&handle) {
            state.on_removed();
        }

        if let Some(ping) = self.pending_pings.remove(&handle) {
            let _ = ping.reply.send(Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "icmp ping cancelled",
            )));
        }

        let _ = self.sockets.remove(handle);
        debug!(
            ?handle,
            active_handles = self.active_handles.len(),
            pending_outbound = self.pending_outbound.len(),
            "netstack socket removed"
        );
    }

    fn fail_all(&mut self, err: SharedIoError) {
        for state in self.tcp_sockets.values() {
            state.on_actor_stopped(err.clone());
        }

        for state in self.udp_sockets.values() {
            state.on_actor_stopped(err.clone());
        }

        for (_, ping) in self.pending_pings.drain() {
            let _ = ping.reply.send(Err(err.to_io_error()));
        }

        self.pending_outbound.clear();
        self.active_handles.clear();
        self.tcp_sockets.clear();
        self.udp_sockets.clear();
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

impl SharedIoError {
    fn new(kind: io::ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: Arc::<str>::from(message.into()),
        }
    }

    fn other(message: impl Into<String>) -> Self {
        Self::new(io::ErrorKind::Other, message)
    }

    fn to_io_error(&self) -> io::Error {
        io::Error::new(self.kind, self.message.to_string())
    }
}

impl TcpSocketState {
    fn new() -> Self {
        Self {
            shared: Mutex::new(TcpSocketShared {
                connect_result: None,
                read_buffer: VecDeque::new(),
                write_buffer: VecDeque::new(),
                read_closed: false,
                send_open: false,
                send_queue_empty: true,
                close_requested: false,
                close_sent: false,
                terminal_error: None,
                connect_waker: None,
                read_waker: None,
                write_waker: None,
                flush_waker: None,
                shutdown_waker: None,
            }),
        }
    }

    fn poll_connect(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut shared = acquire_lock(&self.shared);
        if let Some(result) = &shared.connect_result {
            return Poll::Ready(result.clone().map_err(|err| err.to_io_error()));
        }

        register_waker(&mut shared.connect_waker, cx.waker());
        Poll::Pending
    }

    fn poll_read(&self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let mut shared = acquire_lock(&self.shared);

        if !shared.read_buffer.is_empty() {
            let count = shared.read_buffer.len().min(buf.remaining());
            let chunk: Vec<u8> = shared.read_buffer.drain(..count).collect();
            tracing::debug!(count, "netstack tcp poll_read data");
            buf.put_slice(&chunk);
            return Poll::Ready(Ok(()));
        }

        if let Some(err) = shared.terminal_error.clone() {
            tracing::debug!(error = %err.message, "netstack tcp poll_read terminal_error");
            return Poll::Ready(Err(err.to_io_error()));
        }

        if shared.read_closed {
            tracing::debug!("netstack tcp poll_read EOF (read_closed=true)");
            return Poll::Ready(Ok(()));
        }

        register_waker(&mut shared.read_waker, cx.waker());
        Poll::Pending
    }

    fn poll_write(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let mut shared = acquire_lock(&self.shared);

        if let Some(err) = shared.terminal_error.clone() {
            tracing::debug!(error = %err.message, "netstack tcp poll_write terminal_error");
            return Poll::Ready(Err(err.to_io_error()));
        }

        if shared.close_requested || !shared.send_open {
            tracing::debug!(
                close_requested = shared.close_requested,
                send_open = shared.send_open,
                "netstack tcp poll_write BrokenPipe"
            );
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "tcp stream closed",
            )));
        }

        let available = TCP_BUFFER_SIZE.saturating_sub(shared.write_buffer.len());
        if available == 0 {
            register_waker(&mut shared.write_waker, cx.waker());
            return Poll::Pending;
        }

        let written = available.min(buf.len());
        shared.write_buffer.extend(buf[..written].iter().copied());
        shared.send_queue_empty = false;
        tracing::debug!(
            written,
            write_buffer_len = shared.write_buffer.len(),
            "netstack tcp poll_write buffered"
        );
        Poll::Ready(Ok(written))
    }

    fn poll_flush(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut shared = acquire_lock(&self.shared);

        if let Some(err) = shared.terminal_error.clone() {
            tracing::debug!(error = %err.message, "netstack tcp poll_flush terminal_error");
            return Poll::Ready(Err(err.to_io_error()));
        }

        if shared.write_buffer.is_empty() && shared.send_queue_empty {
            tracing::debug!("netstack tcp poll_flush Ready");
            return Poll::Ready(Ok(()));
        }

        tracing::debug!(
            write_buffer_len = shared.write_buffer.len(),
            send_queue_empty = shared.send_queue_empty,
            "netstack tcp poll_flush Pending"
        );
        register_waker(&mut shared.flush_waker, cx.waker());
        Poll::Pending
    }

    fn poll_shutdown(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut shared = acquire_lock(&self.shared);

        if let Some(err) = shared.terminal_error.clone() {
            tracing::debug!(error = %err.message, "netstack tcp poll_shutdown terminal_error");
            return Poll::Ready(Err(err.to_io_error()));
        }

        shared.close_requested = true;
        if shared.close_sent && shared.write_buffer.is_empty() && shared.send_queue_empty {
            tracing::debug!("netstack tcp poll_shutdown Ready");
            return Poll::Ready(Ok(()));
        }

        tracing::debug!(
            close_sent = shared.close_sent,
            write_buffer_len = shared.write_buffer.len(),
            send_queue_empty = shared.send_queue_empty,
            "netstack tcp poll_shutdown Pending"
        );
        register_waker(&mut shared.shutdown_waker, cx.waker());
        Poll::Pending
    }

    fn on_removed(&self, connect_error: Option<SharedIoError>) {
        let mut shared = acquire_lock(&self.shared);

        if shared.connect_result.is_none() {
            shared.connect_result = Some(Err(connect_error.unwrap_or_else(|| {
                SharedIoError::new(io::ErrorKind::ConnectionAborted, "tcp socket removed")
            })));
        }
        shared.read_closed = true;
        shared.send_open = false;
        shared.send_queue_empty = true;
        shared.close_requested = true;
        shared.close_sent = true;
        wake_all_tcp(&mut shared);
    }

    fn on_actor_stopped(&self, err: SharedIoError) {
        let mut shared = acquire_lock(&self.shared);

        if shared.connect_result.is_none() {
            shared.connect_result = Some(Err(err.clone()));
        }
        if shared.terminal_error.is_none() {
            shared.terminal_error = Some(err);
        }
        shared.read_closed = true;
        shared.send_open = false;
        shared.send_queue_empty = true;
        shared.close_requested = true;
        shared.close_sent = true;
        wake_all_tcp(&mut shared);
    }
}

impl UdpSocketState {
    fn new() -> Self {
        Self {
            shared: Mutex::new(UdpSocketShared {
                recv_queue: VecDeque::new(),
                pending_sends: VecDeque::new(),
                closed: false,
                error: None,
                recv_waker: None,
            }),
        }
    }

    fn poll_recv_from(
        &self,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<(usize, SocketAddr)>> {
        let mut shared = acquire_lock(&self.shared);

        if let Some((packet, addr)) = shared.recv_queue.pop_front() {
            let copied = packet.len().min(buf.len());
            buf[..copied].copy_from_slice(&packet[..copied]);
            return Poll::Ready(Ok((copied, addr)));
        }

        if let Some(err) = shared.error.clone() {
            return Poll::Ready(Err(err.to_io_error()));
        }

        if shared.closed {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "udp socket closed",
            )));
        }

        register_waker(&mut shared.recv_waker, cx.waker());
        Poll::Pending
    }

    fn enqueue_send(
        &self,
        target: SocketAddrV4,
        data: Vec<u8>,
        reply: oneshot::Sender<io::Result<usize>>,
    ) {
        let mut shared = acquire_lock(&self.shared);

        if let Some(err) = shared.error.clone() {
            let _ = reply.send(Err(err.to_io_error()));
            return;
        }

        if shared.closed {
            let _ = reply.send(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "udp socket closed",
            )));
            return;
        }

        shared.pending_sends.push_back(PendingUdpSend {
            data,
            target,
            reply,
        });
    }

    fn on_removed(&self) {
        let mut shared = acquire_lock(&self.shared);
        shared.closed = true;
        fail_udp_sends(
            &mut shared.pending_sends,
            io::ErrorKind::BrokenPipe,
            "udp socket closed",
        );
        take_and_wake(&mut shared.recv_waker);
    }

    fn on_actor_stopped(&self, err: SharedIoError) {
        let mut shared = acquire_lock(&self.shared);
        shared.closed = true;
        if shared.error.is_none() {
            shared.error = Some(err.clone());
        }
        fail_udp_sends(&mut shared.pending_sends, err.kind, err.message.to_string());
        take_and_wake(&mut shared.recv_waker);
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

        match self.state.poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                self.actor.notify_read(self.handle);
                Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
}

impl AsyncWrite for SmolTcpStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        match self.state.poll_write(cx, buf) {
            Poll::Ready(Ok(written)) => {
                self.actor.notify_write(self.handle);
                Poll::Ready(Ok(written))
            }
            other => other,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        match self.state.poll_flush(cx) {
            Poll::Pending => {
                self.actor.notify_flush(self.handle);
                Poll::Pending
            }
            other => other,
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        match self.state.poll_shutdown(cx) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => {
                self.actor.request_close(self.handle);
                Poll::Pending
            }
        }
    }
}

impl Drop for SmolTcpStream {
    fn drop(&mut self) {
        self.actor.remove_socket(self.handle);
    }
}

impl Drop for PendingConnectGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            self.actor.remove_socket(handle);
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
            let target = match target {
                SocketAddr::V4(addr) => addr,
                SocketAddr::V6(_) => return Err(io::Error::other("ipv6 unsupported")),
            };

            let (reply_tx, reply_rx) = oneshot::channel();
            self.actor.send(NetstackCommand::UdpSend {
                handle: self.handle,
                target,
                data: data.to_vec(),
                reply: reply_tx,
            })?;
            recv_actor_reply(reply_rx).await
        })
    }

    fn recv_from<'a>(
        &'a self,
        buf: &'a mut [u8],
    ) -> Pin<Box<dyn std::future::Future<Output = io::Result<(usize, SocketAddr)>> + Send + 'a>>
    {
        Box::pin(async move {
            let result = poll_fn(|cx| self.state.poll_recv_from(cx, buf)).await;
            if result.is_ok() {
                self.actor.notify_read(self.handle);
            }
            result
        })
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.local_addr)
    }
}

impl Drop for SmolUdpSocket {
    fn drop(&mut self) {
        self.actor.remove_socket(self.handle);
    }
}

async fn recv_actor_reply<T>(reply: oneshot::Receiver<io::Result<T>>) -> io::Result<T> {
    match reply.await {
        Ok(result) => result,
        Err(_) => Err(io::Error::other("netstack actor stopped")),
    }
}

fn sync_tcp_socket(socket: &mut tcp::Socket<'static>, state: &Arc<TcpSocketState>) -> bool {
    let mut shared = acquire_lock(&state.shared);
    let mut progressed = false;
    let was_buffer_full = shared.write_buffer.len() >= TCP_BUFFER_SIZE;
    let was_send_open = shared.send_open;
    let was_send_queue_empty = shared.send_queue_empty;
    let was_read_closed = shared.read_closed;
    let was_read_empty = shared.read_buffer.is_empty();
    let was_close_sent = shared.close_sent;

    if shared.connect_result.is_none() {
        if socket.may_send() {
            shared.connect_result = Some(Ok(()));
            take_and_wake(&mut shared.connect_waker);
            progressed = true;
        } else if matches!(socket.state(), tcp::State::Closed | tcp::State::TimeWait) {
            shared.connect_result = Some(Err(SharedIoError::new(
                io::ErrorKind::ConnectionAborted,
                "tcp connect failed",
            )));
            shared.read_closed = true;
            shared.send_open = false;
            wake_all_tcp(&mut shared);
            return true;
        }
    }

    while shared.read_buffer.len() < TCP_BUFFER_SIZE && socket.can_recv() {
        let room = (TCP_BUFFER_SIZE - shared.read_buffer.len()).min(SOCKET_CHUNK_SIZE);
        let mut chunk = vec![0_u8; room];
        match socket.recv_slice(&mut chunk) {
            Ok(received) => {
                if received == 0 {
                    break;
                }
                chunk.truncate(received);
                shared.read_buffer.extend(chunk);
                progressed = true;
            }
            Err(err) => {
                shared.terminal_error = Some(SharedIoError::other(err.to_string()));
                wake_all_tcp(&mut shared);
                return true;
            }
        }
    }

    while socket.can_send() && !shared.write_buffer.is_empty() {
        let count = shared.write_buffer.len().min(SOCKET_CHUNK_SIZE);
        let chunk: Vec<u8> = shared.write_buffer.iter().take(count).copied().collect();
        match socket.send_slice(&chunk) {
            Ok(written) => {
                if written == 0 {
                    break;
                }
                shared.write_buffer.drain(..written);
                tracing::debug!(
                    written,
                    remaining = shared.write_buffer.len(),
                    "netstack sync_tcp_socket drained write_buffer to socket"
                );
                progressed = true;
            }
            Err(err) => {
                tracing::debug!(error = %err, "netstack sync_tcp_socket send_slice error");
                shared.terminal_error = Some(SharedIoError::new(
                    io::ErrorKind::BrokenPipe,
                    err.to_string(),
                ));
                wake_all_tcp(&mut shared);
                return true;
            }
        }
    }

    if shared.close_requested
        && !shared.close_sent
        && shared.write_buffer.is_empty()
        && socket.may_send()
    {
        socket.close();
        shared.close_sent = true;
        progressed = true;
    }

    shared.send_open = socket.may_send();
    shared.send_queue_empty = shared.write_buffer.is_empty() && socket.send_queue() == 0;
    if matches!(shared.connect_result, Some(Ok(()))) && !socket.may_recv() && !socket.can_recv() {
        shared.read_closed = true;
    }

    if was_read_empty && !shared.read_buffer.is_empty() {
        take_and_wake(&mut shared.read_waker);
    }
    if !was_read_closed && shared.read_closed {
        take_and_wake(&mut shared.read_waker);
    }

    let is_buffer_full = shared.write_buffer.len() >= TCP_BUFFER_SIZE;
    if (was_buffer_full && !is_buffer_full)
        || (!was_send_open && shared.send_open)
        || (shared.close_requested && !shared.send_open)
    {
        take_and_wake(&mut shared.write_waker);
    }
    if !was_send_queue_empty && shared.send_queue_empty {
        tracing::debug!(
            "netstack sync_tcp_socket send_queue_empty transition, waking flush+shutdown"
        );
        take_and_wake(&mut shared.flush_waker);
        take_and_wake(&mut shared.shutdown_waker);
    }
    if !was_close_sent && shared.close_sent && shared.send_queue_empty {
        take_and_wake(&mut shared.shutdown_waker);
    }
    if !shared.close_requested && was_send_open && !shared.send_open {
        take_and_wake(&mut shared.write_waker);
    }

    progressed
}

fn sync_udp_socket(socket: &mut udp::Socket<'static>, state: &Arc<UdpSocketState>) -> bool {
    let mut shared = acquire_lock(&state.shared);
    let mut progressed = false;
    let was_recv_empty = shared.recv_queue.is_empty();
    let was_closed = shared.closed;

    while shared.recv_queue.len() < UDP_PACKET_CAPACITY && socket.can_recv() {
        let mut packet = vec![0_u8; UDP_BUFFER_SIZE];
        match socket.recv_slice(&mut packet) {
            Ok((received, metadata)) => {
                packet.truncate(received);
                match socket_addr_from_endpoint(metadata.endpoint) {
                    Ok(addr) => {
                        shared.recv_queue.push_back((packet, addr));
                        progressed = true;
                    }
                    Err(err) => {
                        shared.error = Some(SharedIoError::other(err.to_string()));
                        fail_udp_sends(
                            &mut shared.pending_sends,
                            io::ErrorKind::Other,
                            err.to_string(),
                        );
                        take_and_wake(&mut shared.recv_waker);
                        return true;
                    }
                }
            }
            Err(err) => {
                shared.error = Some(SharedIoError::other(err.to_string()));
                fail_udp_sends(
                    &mut shared.pending_sends,
                    io::ErrorKind::Other,
                    err.to_string(),
                );
                take_and_wake(&mut shared.recv_waker);
                return true;
            }
        }
    }

    while socket.can_send() {
        let Some(pending) = shared.pending_sends.pop_front() else {
            break;
        };
        let size = pending.data.len();
        match socket.send_slice(&pending.data, pending.target) {
            Ok(()) => {
                let _ = pending.reply.send(Ok(size));
                progressed = true;
            }
            Err(err) => {
                let _ = pending.reply.send(Err(io::Error::other(err.to_string())));
                progressed = true;
            }
        }
    }

    if !socket.is_open() {
        shared.closed = true;
        fail_udp_sends(
            &mut shared.pending_sends,
            io::ErrorKind::BrokenPipe,
            "udp socket closed",
        );
    }

    if was_recv_empty && !shared.recv_queue.is_empty() {
        take_and_wake(&mut shared.recv_waker);
    }
    if !was_closed && shared.closed {
        take_and_wake(&mut shared.recv_waker);
    }

    progressed
}

fn wake_all_tcp(shared: &mut TcpSocketShared) {
    take_and_wake(&mut shared.connect_waker);
    take_and_wake(&mut shared.read_waker);
    take_and_wake(&mut shared.write_waker);
    take_and_wake(&mut shared.flush_waker);
    take_and_wake(&mut shared.shutdown_waker);
}

fn register_waker(slot: &mut Option<Waker>, waker: &Waker) {
    match slot {
        Some(existing) if existing.will_wake(waker) => {}
        _ => *slot = Some(waker.clone()),
    }
}

fn take_and_wake(slot: &mut Option<Waker>) {
    if let Some(waker) = slot.take() {
        waker.wake();
    }
}

fn fail_udp_sends(
    pending: &mut VecDeque<PendingUdpSend>,
    kind: io::ErrorKind,
    message: impl Into<String>,
) {
    let message = message.into();
    while let Some(send) = pending.pop_front() {
        let _ = send.reply.send(Err(io::Error::new(kind, message.clone())));
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
    use std::task::{Context, Poll, Waker};

    fn noop_waker() -> Waker {
        Waker::noop().clone()
    }

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

        let snapshot = stack.snapshot().await;
        assert_eq!(
            snapshot.active_handles, 0,
            "timed out connect leaked active socket handles"
        );
    }

    #[tokio::test]
    async fn flush_keeps_driver_responsive_when_outbound_channel_is_full() {
        let (_vpn_tx, vpn_rx) = mpsc::channel(4);
        let (stack_tx, mut stack_rx) = mpsc::channel(1);
        let stack = SmolStack::new(Ipv4Addr::new(10, 0, 0, 8), vpn_rx, stack_tx.clone());

        stack_tx.send(vec![9, 9, 9]).await.unwrap();
        stack.queue_outbound_for_test(vec![1, 2, 3, 4]).await;

        let snapshot = stack.snapshot().await;
        assert_eq!(snapshot.pending_outbound, 1);

        let first = stack_rx.recv().await.unwrap();
        assert_eq!(first, vec![9, 9, 9]);

        let second = tokio::time::timeout(std::time::Duration::from_millis(100), stack_rx.recv())
            .await
            .expect("driver should retry pending outbound packet")
            .expect("stack outbound channel should yield queued packet");
        assert_eq!(second, vec![1, 2, 3, 4]);
    }

    #[tokio::test]
    async fn connect_fails_when_packet_source_closes() {
        let (vpn_tx, vpn_rx) = mpsc::channel(4);
        let (stack_tx, _stack_rx) = mpsc::channel(4);
        let stack = SmolStack::new(Ipv4Addr::new(10, 0, 0, 8), vpn_rx, stack_tx);

        let connect = {
            let stack = stack.clone();
            tokio::spawn(async move {
                stack
                    .connect(SocketAddr::from((Ipv4Addr::new(10, 0, 0, 9), 443)))
                    .await
            })
        };
        tokio::task::yield_now().await;
        drop(vpn_tx);

        let err = tokio::time::timeout(std::time::Duration::from_millis(200), connect)
            .await
            .expect("connect should wake when packet source closes")
            .expect("connect task should join");
        let err = match err {
            Ok(_) => panic!("connect should fail when packet source closes"),
            Err(err) => err,
        };
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[tokio::test]
    async fn connect_fails_when_packet_transport_closes() {
        let (_vpn_tx, vpn_rx) = mpsc::channel(4);
        let (stack_tx, stack_rx) = mpsc::channel(4);
        let stack = SmolStack::new(Ipv4Addr::new(10, 0, 0, 8), vpn_rx, stack_tx);
        drop(stack_rx);

        let connect = {
            let stack = stack.clone();
            tokio::spawn(async move {
                stack
                    .connect(SocketAddr::from((Ipv4Addr::new(10, 0, 0, 9), 443)))
                    .await
            })
        };

        let err = tokio::time::timeout(std::time::Duration::from_millis(200), connect)
            .await
            .expect("connect should wake when packet transport closes")
            .expect("connect task should join");
        let err = match err {
            Ok(_) => panic!("connect should fail when packet transport closes"),
            Err(err) => err,
        };
        assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
    }

    #[tokio::test]
    async fn udp_recv_fails_when_packet_source_closes() {
        let (vpn_tx, vpn_rx) = mpsc::channel(4);
        let (stack_tx, _stack_rx) = mpsc::channel(4);
        let stack = SmolStack::new(Ipv4Addr::new(10, 0, 0, 8), vpn_rx, stack_tx);
        let socket = stack.bind_udp().await.expect("udp bind should succeed");

        drop(vpn_tx);

        let mut buf = [0_u8; 32];
        let err = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            socket.recv_from(&mut buf),
        )
        .await
        .expect("recv should wake when packet source closes")
        .expect_err("recv should fail when packet source closes");
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[tokio::test]
    async fn ping_reports_transport_close_reason() {
        let (_vpn_tx, vpn_rx) = mpsc::channel(4);
        let (stack_tx, stack_rx) = mpsc::channel(4);
        let stack = SmolStack::new(Ipv4Addr::new(10, 0, 0, 8), vpn_rx, stack_tx);
        drop(stack_rx);

        let err = stack
            .ping(Ipv4Addr::new(10, 0, 0, 9))
            .await
            .expect_err("ping should fail when packet transport closes");
        assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn shutdown_waits_for_actor_to_send_close() {
        let state = TcpSocketState::new();
        {
            let mut shared = acquire_lock(&state.shared);
            shared.send_open = true;
            shared.send_queue_empty = true;
        }

        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        assert!(matches!(state.poll_shutdown(&mut cx), Poll::Pending));

        {
            let mut shared = acquire_lock(&state.shared);
            shared.close_sent = true;
        }

        assert!(matches!(state.poll_shutdown(&mut cx), Poll::Ready(Ok(()))));
    }

    #[test]
    fn flush_completes_without_shutdown_once_buffers_drain() {
        let state = TcpSocketState::new();
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        {
            let mut shared = acquire_lock(&state.shared);
            shared.send_open = true;
            shared.send_queue_empty = true;
        }
        assert!(matches!(state.poll_flush(&mut cx), Poll::Ready(Ok(()))));

        {
            let mut shared = acquire_lock(&state.shared);
            shared.write_buffer.push_back(1);
            shared.send_queue_empty = false;
        }
        assert!(matches!(state.poll_flush(&mut cx), Poll::Pending));

        {
            let mut shared = acquire_lock(&state.shared);
            shared.write_buffer.clear();
            shared.send_queue_empty = true;
        }
        assert!(matches!(state.poll_flush(&mut cx), Poll::Ready(Ok(()))));
    }

    #[test]
    fn actor_stop_marks_established_tcp_state_terminal() {
        let state = TcpSocketState::new();
        {
            let mut shared = acquire_lock(&state.shared);
            shared.connect_result = Some(Ok(()));
            shared.send_open = true;
            shared.send_queue_empty = false;
            shared.write_buffer.push_back(1);
        }

        state.on_actor_stopped(SharedIoError::new(
            io::ErrorKind::ConnectionAborted,
            "packet source closed",
        ));

        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        let mut storage = [0_u8; 8];
        let mut read_buf = ReadBuf::new(&mut storage);

        assert!(matches!(
            state.poll_read(&mut cx, &mut read_buf),
            Poll::Ready(Err(err)) if err.kind() == io::ErrorKind::ConnectionAborted
        ));
        assert!(matches!(
            state.poll_write(&mut cx, b"x"),
            Poll::Ready(Err(err)) if err.kind() == io::ErrorKind::ConnectionAborted
        ));
        assert!(matches!(
            state.poll_flush(&mut cx),
            Poll::Ready(Err(err)) if err.kind() == io::ErrorKind::ConnectionAborted
        ));
        assert!(matches!(
            state.poll_shutdown(&mut cx),
            Poll::Ready(Err(err)) if err.kind() == io::ErrorKind::ConnectionAborted
        ));
    }

    #[test]
    fn actor_stop_preserves_buffered_tcp_read_before_error() {
        let state = TcpSocketState::new();
        {
            let mut shared = acquire_lock(&state.shared);
            shared.connect_result = Some(Ok(()));
            shared.read_buffer.extend([1, 2, 3]);
        }

        state.on_actor_stopped(SharedIoError::new(
            io::ErrorKind::ConnectionAborted,
            "packet source closed",
        ));

        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        let mut storage = [0_u8; 8];
        let mut read_buf = ReadBuf::new(&mut storage);

        assert!(matches!(state.poll_read(&mut cx, &mut read_buf), Poll::Ready(Ok(()))));
        assert_eq!(read_buf.filled(), &[1, 2, 3]);
        assert!(matches!(
            state.poll_read(&mut cx, &mut read_buf),
            Poll::Ready(Err(err)) if err.kind() == io::ErrorKind::ConnectionAborted
        ));
    }

    #[test]
    fn actor_stop_preserves_buffered_udp_recv_before_error() {
        let state = UdpSocketState::new();
        let addr = SocketAddr::from((Ipv4Addr::new(10, 0, 0, 9), 53));
        {
            let mut shared = acquire_lock(&state.shared);
            shared.recv_queue.push_back((vec![9, 8, 7], addr));
        }

        state.on_actor_stopped(SharedIoError::new(
            io::ErrorKind::ConnectionAborted,
            "packet source closed",
        ));

        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        let mut buf = [0_u8; 8];

        assert!(matches!(
            state.poll_recv_from(&mut cx, &mut buf),
            Poll::Ready(Ok((3, recv_addr))) if recv_addr == addr && buf[..3] == [9, 8, 7]
        ));
        assert!(matches!(
            state.poll_recv_from(&mut cx, &mut buf),
            Poll::Ready(Err(err)) if err.kind() == io::ErrorKind::ConnectionAborted
        ));
    }
}
