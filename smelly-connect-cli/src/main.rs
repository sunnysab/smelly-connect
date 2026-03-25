fn main() {
    let cli = smelly_connect_cli::cli::Cli::parse_from(std::env::args_os());
    let config_path = cli.config.clone();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build runtime");
    let result = rt.block_on(async move {
        match cli.command {
            smelly_connect_cli::cli::Command::Proxy(command) => {
                smelly_connect_cli::commands::proxy::run_proxy(&config_path, &command).await
            }
            smelly_connect_cli::cli::Command::Inspect(cmd) => match cmd {
                smelly_connect_cli::cli::InspectCommand::Route { host, port } => {
                    let output = smelly_connect_cli::commands::inspect::run_route_with_config(
                        &config_path,
                        &host,
                        port,
                    )
                    .await?;
                    println!("{output}");
                    Ok(())
                }
                smelly_connect_cli::cli::InspectCommand::Session => {
                    let output = smelly_connect_cli::commands::inspect::run_session_with_config(
                        &config_path,
                    )
                    .await?;
                    println!("{output}");
                    Ok(())
                }
            },
            smelly_connect_cli::cli::Command::Test(cmd) => match cmd {
                smelly_connect_cli::cli::TestCommand::Tcp { target } => {
                    let output = smelly_connect_cli::commands::test::run_tcp_with_config(
                        &config_path,
                        &target,
                    )
                    .await?;
                    println!("{output}");
                    Ok(())
                }
                smelly_connect_cli::cli::TestCommand::Icmp { target } => {
                    let output = smelly_connect_cli::commands::test::run_icmp_with_config(
                        &config_path,
                        &target,
                    )
                    .await?;
                    println!("{output}");
                    Ok(())
                }
                smelly_connect_cli::cli::TestCommand::Http { url } => {
                    let output = smelly_connect_cli::commands::test::run_http_with_config(
                        &config_path,
                        &url,
                    )
                    .await?;
                    println!("{output}");
                    Ok(())
                }
            },
        }
    });

    if let Err(err) = result {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
