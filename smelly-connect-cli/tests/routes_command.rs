#[tokio::test]
async fn routes_command_reports_domain_ip_and_static_dns_rules() {
    let output = smelly_connect_cli::commands::routes::run_routes_for_test(
        "127.0.0.1:19090",
        r#"{
            "total_nodes":1,
            "nodes":[
                {
                    "name":"acct-01",
                    "state":"ready",
                    "routes":{
                        "domain_rules":[
                            {"domain":"jwxt.sit.edu.cn","port_min":1,"port_max":65535,"protocol":"all"}
                        ],
                        "ip_rules":[
                            {"ip_min":"10.0.0.8","ip_max":"10.0.0.8","port_min":1,"port_max":65535,"protocol":"all"}
                        ],
                        "static_dns":[
                            {"host":"jwxt.sit.edu.cn","ip":"10.0.0.8"}
                        ]
                    },
                    "local_routes":{
                        "domain_rules":[
                            {"domain":".foo.edu.cn","port_min":443,"port_max":443,"protocol":"tcp"}
                        ],
                        "ip_rules":[
                            {"ip_min":"42.62.107.1","ip_max":"42.62.107.254","port_min":1,"port_max":65535,"protocol":"all"}
                        ],
                        "static_dns":[]
                    }                    
                }
            ]
        }"#,
    )
    .await
    .unwrap();

    assert!(output.contains("management=127.0.0.1:19090"));
    assert!(output.contains("total_nodes=1"));
    assert!(output.contains("account=acct-01 state=ready"));
    assert!(output.contains("remote domain jwxt.sit.edu.cn ports=1-65535 protocol=all"));
    assert!(output.contains("remote ip 10.0.0.8-10.0.0.8 ports=1-65535 protocol=all"));
    assert!(output.contains("remote dns jwxt.sit.edu.cn=10.0.0.8"));
    assert!(output.contains("local domain .foo.edu.cn ports=443-443 protocol=tcp"));
    assert!(output.contains("local ip 42.62.107.1-42.62.107.254 ports=1-65535 protocol=all"));
}

#[test]
fn routes_command_returns_typed_error_when_management_is_disabled() {
    let path = std::env::temp_dir().join("smelly-connect-cli-routes-no-management.toml");
    std::fs::write(
        &path,
        r#"
        [vpn]
        server = "vpn1.sit.edu.cn"

        [pool]
        prewarm = 1
        connect_timeout_secs = 20
        healthcheck_interval_secs = 60

        [[accounts]]
        name = "acct-01"
        username = "user1"
        password = "pass1"

        [proxy.http]
        enabled = true
        listen = "127.0.0.1:8080"

        [proxy.socks5]
        enabled = false
        listen = "127.0.0.1:1080"
        "#,
    )
    .unwrap();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let err = rt
        .block_on(smelly_connect_cli::commands::routes::run_routes_with_config_typed(
            &path,
        ))
        .unwrap_err();
    assert!(matches!(err, smelly_connect_cli::error::CliError::Command(_)));
    let _ = std::fs::remove_file(path);
}
