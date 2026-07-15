fn main() {
    let cli = smelly_connect_cli::cli::Cli::parse_from(std::env::args_os());
    let config_path = cli.config.clone();
    let config = match smelly_connect_cli::config::load_typed(&config_path) {
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
                let result =
                    smelly_connect_cli::commands::proxy::run_proxy(&config, &command, async {
                        tokio::signal::ctrl_c()
                            .await
                            .expect("install ctrl-c handler");
                        tracing::info!("received SIGINT, shutting down gracefully...");
                    })
                    .await;
                tracing::info!("shutdown complete");
                result
            }
            smelly_connect_cli::cli::Command::Routes => {
                let output = smelly_connect_cli::commands::routes::run_routes(&config).await?;
                println!("{output}");
                Ok(())
            }
            smelly_connect_cli::cli::Command::Status(command) => {
                let output = smelly_connect_cli::commands::status::run_status(
                    &config,
                    command.management_api.as_deref(),
                )
                .await?;
                println!("{output}");
                Ok(())
            }
            smelly_connect_cli::cli::Command::Inspect(cmd) => match cmd {
                smelly_connect_cli::cli::InspectCommand::Route { host, port } => {
                    let output =
                        smelly_connect_cli::commands::inspect::run_route(&config, &host, port)
                            .await?;
                    println!("{output}");
                    Ok(())
                }
                smelly_connect_cli::cli::InspectCommand::Session => {
                    let output =
                        smelly_connect_cli::commands::inspect::run_session(&config).await?;
                    println!("{output}");
                    Ok(())
                }
            },
            smelly_connect_cli::cli::Command::Test(cmd) => match cmd {
                smelly_connect_cli::cli::TestCommand::Tcp { target } => {
                    let output =
                        smelly_connect_cli::commands::test::run_tcp(&config, &target).await?;
                    println!("{output}");
                    Ok(())
                }
                smelly_connect_cli::cli::TestCommand::Icmp { target } => {
                    let output =
                        smelly_connect_cli::commands::test::run_icmp(&config, &target).await?;
                    println!("{output}");
                    Ok(())
                }
                smelly_connect_cli::cli::TestCommand::Http { url } => {
                    let output =
                        smelly_connect_cli::commands::test::run_http(&config, &url).await?;
                    println!("{output}");
                    Ok(())
                }
                smelly_connect_cli::cli::TestCommand::LegacyProbe => {
                    let output =
                        smelly_connect_cli::commands::test::run_legacy_probe(&config).await?;
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
