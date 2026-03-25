fn main() {
    let cli = smelly_connect_cli::cli::Cli::parse_from(std::env::args_os());
    match cli.command {
        smelly_connect_cli::cli::Command::Proxy => {
            eprintln!("smelly-connect-cli proxy is not wired yet");
        }
        smelly_connect_cli::cli::Command::Inspect(cmd) => {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build runtime");
            rt.block_on(async move {
                match cmd {
                    smelly_connect_cli::cli::InspectCommand::Route { host, port } => {
                        let _ = smelly_connect_cli::commands::inspect::run_route(&host, port).await;
                    }
                    smelly_connect_cli::cli::InspectCommand::Session => {
                        let _ = smelly_connect_cli::commands::inspect::run_session().await;
                    }
                }
            });
        }
    }
}
