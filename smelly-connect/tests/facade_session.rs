use smelly_connect::{EasyConnectClient, Session};

#[test]
fn facade_types_are_exported() {
    let _ = std::any::TypeId::of::<EasyConnectClient>();
    let _ = std::any::TypeId::of::<Session>();
}
