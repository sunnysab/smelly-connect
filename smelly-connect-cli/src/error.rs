use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliError {
    Config(String),
    Logging(String),
    Command(String),
}

impl Display for CliError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(message) | Self::Logging(message) | Self::Command(message) => {
                f.write_str(message)
            }
        }
    }
}

impl std::error::Error for CliError {}

impl CliError {
    pub fn is_config_error(&self) -> bool {
        matches!(self, Self::Config(_))
    }
}
