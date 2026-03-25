use smelly_connect::{EasyConnectConfig, TargetAddr};

#[test]
fn public_api_smoke_compiles() {
    let _cfg = EasyConnectConfig::new("rvpn.example.com", "user", "pass");
    let _target = TargetAddr::from(("libdb.zju.edu.cn", 443));
}
