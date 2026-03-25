use crate::cli::ProxyCommand;

pub async fn run_proxy(_command: &ProxyCommand) -> Result<(), String> {
    eprintln!("smelly-connect-cli proxy foreground mode is not fully wired yet");
    Ok(())
}
