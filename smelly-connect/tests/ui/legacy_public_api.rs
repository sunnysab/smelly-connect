use smelly_connect::auth::control::{request_ip_via_tunnel_with_conn, run_control_plane};
use smelly_connect::session::EasyConnectSession;

fn main() {
    let _ = run_control_plane;
    let _ = request_ip_via_tunnel_with_conn;
    let _ = EasyConnectSession::with_legacy_data_plane;
    let _ = EasyConnectSession::spawn_packet_device;
}
