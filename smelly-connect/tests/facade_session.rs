use smelly_connect::{ConnectTarget, EasyConnectClient, KeepalivePolicy, Session, SessionInfo};

#[test]
fn facade_types_are_exported() {
    let _ = std::any::TypeId::of::<EasyConnectClient>();
    let _ = std::any::TypeId::of::<Session>();
}

#[test]
fn connect_target_accepts_host_port_and_socket_addr() {
    let host = ConnectTarget::from(("jwxt.sit.edu.cn", 443));
    assert_eq!(host.port(), 443);

    let socket = ConnectTarget::from("10.0.0.8:443".parse::<std::net::SocketAddr>().unwrap());
    assert_eq!(socket.host(), "10.0.0.8");
}

#[test]
fn session_info_exposes_client_ip() {
    let info = SessionInfo::new("10.0.0.8".parse().unwrap());
    assert_eq!(info.client_ip().to_string(), "10.0.0.8");
}

#[test]
fn keepalive_policy_can_hold_target_and_interval() {
    let policy =
        KeepalivePolicy::icmp(("jwxt.sit.edu.cn", 443), std::time::Duration::from_secs(60));
    match policy {
        KeepalivePolicy::Disabled => panic!("expected icmp policy"),
        KeepalivePolicy::Icmp { target, interval } => {
            assert_eq!(target.host(), "jwxt.sit.edu.cn");
            assert_eq!(interval, std::time::Duration::from_secs(60));
        }
    }
}

#[test]
fn easyconnect_client_builder_collects_credentials() {
    let _client = EasyConnectClient::builder("rvpn.example.com")
        .credentials("user", "pass")
        .build()
        .unwrap();
}

#[test]
fn crate_version_is_0_2_0() {
    assert_eq!(env!("CARGO_PKG_VERSION"), "0.2.0");
}
