use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};
use std::ffi::OsString;

#[derive(Debug, Clone, Parser)]
#[command(name = "smelly-connect-cli")]
pub struct Cli {
    #[arg(long, default_value = "config.toml", global = true)]
    pub config: PathBuf,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    Proxy(ProxyCommand),
    Routes,
    Status,
    #[command(subcommand)]
    Inspect(InspectCommand),
    #[command(subcommand)]
    Test(TestCommand),
}

#[derive(Debug, Clone, Args)]
pub struct ProxyCommand {
    #[arg(long)]
    pub listen_http: Option<String>,
    #[arg(long)]
    pub listen_socks5: Option<String>,
    #[arg(long)]
    pub prewarm: Option<usize>,
    #[arg(long)]
    pub keepalive_host: Option<String>,
    #[arg(long)]
    pub allow_all: bool,
}

#[derive(Debug, Clone, Subcommand)]
pub enum InspectCommand {
    Route {
        host: String,
        #[arg(default_value_t = 443)]
        port: u16,
    },
    Session,
}

#[derive(Debug, Clone, Subcommand)]
pub enum TestCommand {
    Tcp { target: String },
    Icmp { target: String },
    Http { url: String },
}

impl Cli {
    pub fn parse_from<I, T>(itr: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        <Self as Parser>::parse_from(itr)
    }

    pub fn config_path(&self) -> &Path {
        &self.config
    }
}
