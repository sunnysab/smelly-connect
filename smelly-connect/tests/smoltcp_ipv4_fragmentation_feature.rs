#[test]
fn enables_smoltcp_ipv4_fragmentation_support() {
    let manifest = include_str!("../Cargo.toml");
    assert!(
        manifest.contains("\"proto-ipv4-fragmentation\""),
        "smoltcp must enable proto-ipv4-fragmentation so fragmented VPN TCP packets can be reassembled"
    );
}
