use std::net::Ipv4Addr;

/// Fixed TLS-looking ClientHello sent by svpnservice (0x52 bytes).
/// Mimics a TLS 1.0 ClientHello with cipher 0x0039.
pub const FIXED_SSL_SYN: &[u8; 0x52] = &[
    0x16, 0x03, 0x01, 0x00, 0x4d, 0x01, 0x00, 0x00, 0x49, 0x03, 0x01, 0x46, 0x83, 0x74, 0x24,
    0x01, 0x43, 0x16, 0x29, 0x1f, 0x95, 0x3e, 0xc9, 0x63, 0xb9, 0xfc, 0xb6, 0x16, 0x00, 0xaa,
    0x20, 0x15, 0xeb, 0x75, 0x07, 0x59, 0x40, 0x8b, 0x8d, 0x0c, 0xe1, 0x60, 0x43, 0x20, 0xb1,
    0x65, 0xf6, 0x7f, 0x5a, 0x6d, 0x2c, 0x54, 0xd2, 0x71, 0x69, 0x97, 0x72, 0x58, 0x65, 0xc3,
    0x40, 0x9a, 0x8a, 0x0f, 0xcd, 0x81, 0xd8, 0x2e, 0xdb, 0x7b, 0x8f, 0xf7, 0x1b, 0xc1, 0x80,
    0xc6, 0x00, 0x02, 0x00, 0x39, 0x01, 0x00,
];

/// Fixed TLS-looking ChangeCipherSpec + Finished sent by svpnservice (0x2B bytes).
pub const FIXED_SSL_ACK: &[u8; 0x2b] = &[
    0x14, 0x03, 0x01, 0x00, 0x01, 0x01, 0x16, 0x03, 0x01, 0x00, 0x20, 0x2b, 0xd1, 0xdf, 0x0e,
    0xe2, 0x4c, 0xda, 0x7a, 0x36, 0x0e, 0xde, 0x24, 0x81, 0xb9, 0x79, 0x32, 0x67, 0x8f, 0xf2,
    0x88, 0x8c, 0xef, 0xa3, 0x7e, 0x50, 0x1c, 0x09, 0x6b, 0x09, 0x9c, 0x5a, 0x0d,
];

pub const SERVER_SSL_ACK_LEN: usize = 0x7a;
pub const CLIENT_MSG_LEN: usize = 0x4c;
pub const SERVER_MSG_LEN: usize = 0x28;

// ClientMsg command types
pub const CMD_NEW_CONNECT: u32 = 0;
pub const CMD_RECONNECT: u32 = 1;
pub const CMD_HEARTBEAT: u32 = 3;
pub const CMD_MAKE_RECV_TUNNEL: u32 = 6;
pub const CMD_MAKE_SEND_TUNNEL: u32 = 5;

// ServerMsg command types
pub const SERVER_SEND_IP: u32 = 0;
pub const SERVER_RESET: u32 = 3;
pub const SERVER_RECOVERED: u32 = 4;
pub const SERVER_SHUTDOWN: u32 = 8;
pub const SERVER_IPKICK: u32 = 14;
pub const SERVER_HEARTBEAT: u32 = 15;

/// Build a 0x4C-byte ClientMsg with JJYY magic.
pub fn build_client_msg(cmd_type: u32, peer_sockaddr: &[u8; 16], extra: u32) -> [u8; CLIENT_MSG_LEN] {
    let mut buf = [0u8; CLIENT_MSG_LEN];
    // TLS Application Data record header
    buf[0] = 0x17;
    buf[1] = 0x03;
    buf[2] = 0x01;
    buf[4] = 0x3c;
    // JJYY magic
    buf[8] = b'J';
    buf[9] = b'J';
    buf[10] = b'Y';
    buf[11] = b'Y';
    // Command type (little-endian)
    buf[0x0c..0x10].copy_from_slice(&cmd_type.to_le_bytes());
    // Peer sockaddr_in at 0x30
    buf[0x30..0x40].copy_from_slice(peer_sockaddr);
    // Extra fields at 0x44 and 0x48
    buf[0x44..0x48].copy_from_slice(&extra.to_le_bytes());
    buf
}

/// Build a ClientMsg for NEWCONNECT (first connection).
pub fn build_new_connect_msg(peer_sockaddr: &[u8; 16]) -> [u8; CLIENT_MSG_LEN] {
    let mut buf = build_client_msg(CMD_NEW_CONNECT, peer_sockaddr, 0);
    // For NEWCONNECT/RECONNECT, extra2 at 0x48 = 0xFFFFFFFF
    buf[0x48..0x4c].copy_from_slice(&0xFFFFFFFF_u32.to_le_bytes());
    buf
}

/// Build a ClientMsg for HEARTBEAT.
pub fn build_heartbeat_msg(peer_sockaddr: &[u8; 16]) -> [u8; CLIENT_MSG_LEN] {
    build_client_msg(CMD_HEARTBEAT, peer_sockaddr, 0)
}

/// Build a ClientMsg for data tunnel setup (send or recv).
/// `tun_ip` is the virtual tunnel IP in network byte order.
pub fn build_tunnel_msg(cmd_type: u32, peer_sockaddr: &[u8; 16], tun_ip: Ipv4Addr) -> [u8; CLIENT_MSG_LEN] {
    let ip_u32 = u32::from_be_bytes(tun_ip.octets());
    build_client_msg(cmd_type, peer_sockaddr, ip_u32.to_be())
}

/// Parsed ServerMsg (0x28 bytes, AABB magic).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerMsg {
    pub cmd_type: u32,
    pub a: u32,
    pub b: u32,
    pub c: u32,
    pub d: u32,
    pub e: u32,
}

impl ServerMsg {
    /// Parse a 0x28-byte server message with AABB magic.
    pub fn parse(buf: &[u8; SERVER_MSG_LEN]) -> Result<Self, CommandProtocolError> {
        if &buf[0..4] != b"AABB" {
            return Err(CommandProtocolError::BadMagic([buf[0], buf[1], buf[2], buf[3]]));
        }
        Ok(Self {
            cmd_type: u32::from_le_bytes(buf[4..8].try_into().unwrap()),
            a: u32::from_le_bytes(buf[8..12].try_into().unwrap()),
            b: u32::from_le_bytes(buf[12..16].try_into().unwrap()),
            c: u32::from_le_bytes(buf[16..20].try_into().unwrap()),
            d: u32::from_le_bytes(buf[20..24].try_into().unwrap()),
            e: u32::from_le_bytes(buf[24..28].try_into().unwrap()),
        })
    }
}

/// Parsed SEND_IP response (server cmd_type == 0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendIpInfo {
    /// Virtual tunnel IP (network byte order).
    pub tun_ip: Ipv4Addr,
    /// Encryption type (0 = RC4).
    pub enc_type: u32,
    /// Peer LAN IP (network byte order).
    pub peer_lan_ip: Ipv4Addr,
    /// UDP port (host byte order).
    pub udp_port: u32,
    /// Compression flag (0=none, 3=LZO, 5=ZLIB).
    pub zip_flag: u32,
}

impl SendIpInfo {
    pub fn from_server_msg(msg: &ServerMsg) -> Result<Self, CommandProtocolError> {
        if msg.cmd_type != SERVER_SEND_IP {
            return Err(CommandProtocolError::UnexpectedCommand {
                got: msg.cmd_type,
                expected: SERVER_SEND_IP,
            });
        }
        Ok(Self {
            tun_ip: Ipv4Addr::from(msg.a.to_be_bytes()),
            enc_type: msg.b,
            peer_lan_ip: Ipv4Addr::from(msg.c.to_be_bytes()),
            udp_port: msg.d,
            zip_flag: msg.e,
        })
    }
}

/// Derive the 16-byte sockaddr_in for a server address.
/// Format: [family=2(LE), port(BE), ipv4(BE), padding(8 zeros)]
pub fn derive_peer_sockaddr(host: &str, port: u16) -> [u8; 16] {
    let mut out = [0u8; 16];
    // AF_INET = 2, stored as little-endian u16
    out[0..2].copy_from_slice(&2u16.to_le_bytes());
    out[2..4].copy_from_slice(&port.to_be_bytes());
    if let Ok(ip) = host.parse::<Ipv4Addr>() {
        out[4..8].copy_from_slice(&ip.octets());
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub enum CommandProtocolError {
    BadMagic([u8; 4]),
    UnexpectedCommand { got: u32, expected: u32 },
    Io(std::io::ErrorKind),
}

impl std::fmt::Display for CommandProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadMagic(m) => write!(f, "bad server message magic: {:02x?}", m),
            Self::UnexpectedCommand { got, expected } => {
                write!(f, "unexpected command: got {got}, expected {expected}")
            }
            Self::Io(kind) => write!(f, "io error: {kind}"),
        }
    }
}

impl std::error::Error for CommandProtocolError {}

impl From<std::io::Error> for CommandProtocolError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.kind())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_msg_layout() {
        let peer = [0u8; 16];
        let msg = build_new_connect_msg(&peer);
        assert_eq!(&msg[0..5], &[0x17, 0x03, 0x01, 0x00, 0x3c]);
        assert_eq!(&msg[8..12], b"JJYY");
        assert_eq!(u32::from_le_bytes(msg[0x0c..0x10].try_into().unwrap()), 0);
        assert_eq!(u32::from_le_bytes(msg[0x48..0x4c].try_into().unwrap()), 0xFFFFFFFF);
        assert_eq!(msg.len(), 0x4c);
    }

    #[test]
    fn client_msg_heartbeat() {
        let peer = derive_peer_sockaddr("10.0.0.1", 443);
        let msg = build_heartbeat_msg(&peer);
        assert_eq!(u32::from_le_bytes(msg[0x0c..0x10].try_into().unwrap()), CMD_HEARTBEAT);
    }

    #[test]
    fn client_msg_tunnel_type() {
        let peer = [0u8; 16];
        let tun_ip: Ipv4Addr = "10.10.0.100".parse().unwrap();
        let msg = build_tunnel_msg(CMD_MAKE_SEND_TUNNEL, &peer, tun_ip);
        assert_eq!(u32::from_le_bytes(msg[0x0c..0x10].try_into().unwrap()), CMD_MAKE_SEND_TUNNEL);
        // Extra should be ntohl(tunIP) stored as LE
        let extra = u32::from_le_bytes(msg[0x44..0x48].try_into().unwrap());
        assert_eq!(extra, u32::from_be_bytes(tun_ip.octets()).to_be());
    }

    #[test]
    fn server_msg_parse_send_ip() {
        let mut buf = [0u8; SERVER_MSG_LEN];
        buf[0..4].copy_from_slice(b"AABB");
        buf[4..8].copy_from_slice(&0u32.to_le_bytes()); // cmd_type = SEND_IP
        buf[8..12].copy_from_slice(&0x0A0A0064u32.to_le_bytes()); // tunIP = 10.10.0.100
        buf[12..16].copy_from_slice(&0u32.to_le_bytes()); // encType
        buf[16..20].copy_from_slice(&0x0A0A0001u32.to_le_bytes()); // peerLanIP = 10.10.0.1
        buf[20..24].copy_from_slice(&12345u32.to_le_bytes()); // udpPort
        buf[24..28].copy_from_slice(&0u32.to_le_bytes()); // zipFlag

        let msg = ServerMsg::parse(&buf).unwrap();
        assert_eq!(msg.cmd_type, SERVER_SEND_IP);

        let info = SendIpInfo::from_server_msg(&msg).unwrap();
        assert_eq!(info.tun_ip, Ipv4Addr::new(10, 10, 0, 100));
        assert_eq!(info.enc_type, 0);
        assert_eq!(info.peer_lan_ip, Ipv4Addr::new(10, 10, 0, 1));
        assert_eq!(info.udp_port, 12345);
        assert_eq!(info.zip_flag, 0);
    }

    #[test]
    fn server_msg_bad_magic() {
        let mut buf = [0u8; SERVER_MSG_LEN];
        buf[0..4].copy_from_slice(b"XXXX");
        assert!(matches!(
            ServerMsg::parse(&buf),
            Err(CommandProtocolError::BadMagic(_))
        ));
    }

    #[test]
    fn derive_peer_sockaddr_format() {
        let addr = derive_peer_sockaddr("192.168.1.1", 8443);
        assert_eq!(addr[0], 2); // AF_INET
        assert_eq!(addr[1], 0);
        assert_eq!(u16::from_be_bytes(addr[2..4].try_into().unwrap()), 8443);
        assert_eq!(&addr[4..8], &[192, 168, 1, 1]);
    }
}
