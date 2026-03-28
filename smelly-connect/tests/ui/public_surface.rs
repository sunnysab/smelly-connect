use smelly_connect::protocol::legacy_tls::build_easyconnect_connector;
use smelly_connect::runtime::control_plane::types::ControlPlaneState;
use smelly_connect::test_support::session::fake_session_without_match;

fn main() {
    let _ = build_easyconnect_connector;
    let _ = std::mem::size_of::<ControlPlaneState>();
    let _ = fake_session_without_match;
}
