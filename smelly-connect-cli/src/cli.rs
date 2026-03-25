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
