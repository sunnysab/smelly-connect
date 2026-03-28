fn main() {
    let cli = smelly_connect_cli::cli::Cli::parse_from(std::env::args_os());
    let config_path = cli.config.clone();
    let config = match smelly_connect_cli::config::load(&config_path) {
        Ok(config) => config,
        Err(err) => {
            smelly_connect_cli::logging::emit_fatal_stderr(&format!(
                "configuration load failed path={} error={err}",
                config_path.display()
            ));
            std::process::exit(1);
        }
    };
    let _logging_guard = match smelly_connect_cli::logging::init_logging(&config.logging) {
        Ok(guard) => Some(guard),
        Err(err) => {
            smelly_connect_cli::logging::emit_fatal_stderr(&format!(
                "logging initialization failed path={} error={err}",
                config_path.display()
            ));
            std::process::exit(1);
        }
    };
    tracing::info!(
        config = %config_path.display(),
        mode = %config.logging.mode.as_str(),
        level = %config.logging.level.as_str(),
        "cli startup"
    );
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build runtime");
    let result = rt.block_on(async move {
        match cli.command {
            smelly_connect_cli::cli::Command::Proxy(command) => {
                smelly_connect_cli::commands::proxy::run_proxy(&config_path, &command).await
            }
            smelly_connect_cli::cli::Command::Routes => {
                let output =
                    smelly_connect_cli::commands::routes::run_routes_with_config(&config_path)
                        .await?;
                println!("{output}");
                Ok(())
            }
            smelly_connect_cli::cli::Command::Status => {
                let output =
                    smelly_connect_cli::commands::status::run_status_with_config(&config_path)
                        .await?;
                println!("{output}");
                Ok(())
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
        tracing::error!(error = %err, "command failed");
        eprintln!("{err}");
        std::process::exit(1);
    }
}
