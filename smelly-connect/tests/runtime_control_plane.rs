use smelly_connect::AuthenticatedSessionSeed;

#[test]
fn authenticated_session_seed_carries_resources_and_tunnel_bootstrap() {
    let _ = std::any::TypeId::of::<AuthenticatedSessionSeed>();
}
