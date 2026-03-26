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
    assert!(output.contains("domain jwxt.sit.edu.cn ports=1-65535 protocol=all"));
    assert!(output.contains("ip 10.0.0.8-10.0.0.8 ports=1-65535 protocol=all"));
    assert!(output.contains("dns jwxt.sit.edu.cn=10.0.0.8"));
}
