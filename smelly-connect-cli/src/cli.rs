use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
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
    Proxy,
    #[command(subcommand)]
    Inspect(InspectCommand),
    #[command(subcommand)]
    Test(TestCommand),
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
