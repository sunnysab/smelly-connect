use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};

use super::command::{
    self, CMD_MAKE_RECV_TUNNEL, CMD_MAKE_SEND_TUNNEL, FIXED_SSL_ACK, FIXED_SSL_SYN,
    SERVER_MSG_LEN, SERVER_SSL_ACK_LEN, ServerMsg,
};
use super::rc4::RC4State;

/// A connected data tunnel (send or recv direction).
pub struct DataTunnel {
    stream: TcpStream,
    rc4: RC4State,
}

impl DataTunnel {
    pub fn stream(&self) -> &TcpStream {
        &self.stream
    }

    pub fn into_parts(self) -> (TcpStream, RC4State) {
        (self.stream, self.rc4)
    }

    pub fn rc4_state(&mut self) -> &mut RC4State {
        &mut self.rc4
    }
}

/// Establish the TLS-looking handshake on a TCP connection.
/// Returns the connection after handshake is complete.
fn perform_tls_handshake(addr: SocketAddr) -> Result<TcpStream, DataTunnelError> {
    let mut stream = TcpStream::connect(addr).map_err(DataTunnelError::Io)?;

    // Send ssl syn
    stream.write_all(FIXED_SSL_SYN).map_err(DataTunnelError::Io)?;

    // Read server ssl ack (0x7A bytes)
    let mut server_ack = [0u8; SERVER_SSL_ACK_LEN];
    stream.read_exact(&mut server_ack).map_err(DataTunnelError::Io)?;

    // Send ssl ack
    stream.write_all(FIXED_SSL_ACK).map_err(DataTunnelError::Io)?;

    Ok(stream)
}

/// Connect a data tunnel (send or recv direction).
///
/// 1. TCP connect
/// 2. TLS-looking handshake (ssl_syn → ssl_ack ← → ssl_ack)
/// 3. Send ClientMsg with tunnel type
/// 4. Read ServerMsg and validate reply
pub fn connect_data_tunnel(
    addr: SocketAddr,
    cmd_type: u32,
    peer_sockaddr: &[u8; 16],
    tun_ip: Ipv4Addr,
    rc4_key: &[u8; 16],
    expected_reply_cmd: u32,
) -> Result<DataTunnel, DataTunnelError> {
    let mut stream = perform_tls_handshake(addr)?;

    // Send tunnel setup ClientMsg
    let msg = command::build_tunnel_msg(cmd_type, peer_sockaddr, tun_ip);
    stream.write_all(&msg).map_err(DataTunnelError::Io)?;

    // Read ServerMsg reply
    let mut reply = [0u8; SERVER_MSG_LEN];
    stream.read_exact(&mut reply).map_err(DataTunnelError::Io)?;

    let server_msg = ServerMsg::parse(&reply)?;
    if server_msg.cmd_type != expected_reply_cmd {
        return Err(DataTunnelError::UnexpectedReply {
            got: server_msg.cmd_type,
            expected: expected_reply_cmd,
        });
    }

    Ok(DataTunnel {
        stream,
        rc4: RC4State::new(rc4_key),
    })
}

/// Connect a send data tunnel (type 5, expects reply cmd_type == 2).
pub fn connect_send_tunnel(
    addr: SocketAddr,
    peer_sockaddr: &[u8; 16],
    tun_ip: Ipv4Addr,
    rc4_key: &[u8; 16],
) -> Result<DataTunnel, DataTunnelError> {
    connect_data_tunnel(
        addr,
        CMD_MAKE_SEND_TUNNEL,
        peer_sockaddr,
        tun_ip,
        rc4_key,
        2, // Server SEND OK
    )
}

/// Connect a recv data tunnel (type 6, expects reply cmd_type == 1).
pub fn connect_recv_tunnel(
    addr: SocketAddr,
    peer_sockaddr: &[u8; 16],
    tun_ip: Ipv4Addr,
    rc4_key: &[u8; 16],
) -> Result<DataTunnel, DataTunnelError> {
    connect_data_tunnel(
        addr,
        CMD_MAKE_RECV_TUNNEL,
        peer_sockaddr,
        tun_ip,
        rc4_key,
        1, // Server RECV OK
    )
}

#[derive(Debug)]
pub enum DataTunnelError {
    Io(std::io::Error),
    CommandProtocol(command::CommandProtocolError),
    UnexpectedReply { got: u32, expected: u32 },
}

impl std::fmt::Display for DataTunnelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::CommandProtocol(e) => write!(f, "command protocol error: {e}"),
            Self::UnexpectedReply { got, expected } => {
                write!(f, "unexpected reply command: got {got}, expected {expected}")
            }
        }
    }
}

impl std::error::Error for DataTunnelError {}

impl From<command::CommandProtocolError> for DataTunnelError {
    fn from(e: command::CommandProtocolError) -> Self {
        Self::CommandProtocol(e)
    }
}
