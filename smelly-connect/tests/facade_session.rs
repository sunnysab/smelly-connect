use smelly_connect::{ConnectTarget, Session};

#[test]
fn session_type_is_exported() {
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
fn crate_version_is_0_6_1() {
    assert_eq!(env!("CARGO_PKG_VERSION"), "0.6.1");
}
