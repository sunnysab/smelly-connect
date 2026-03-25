#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetAddr {
    HostPort { host: String, port: u16 },
}

impl TargetAddr {
    pub fn host(&self) -> &str {
        match self {
            Self::HostPort { host, .. } => host,
        }
    }

    pub fn port(&self) -> u16 {
        match self {
            Self::HostPort { port, .. } => *port,
        }
    }
}

impl From<(&str, u16)> for TargetAddr {
    fn from(value: (&str, u16)) -> Self {
        Self::HostPort {
            host: value.0.to_string(),
            port: value.1,
        }
    }
}

impl From<(String, u16)> for TargetAddr {
    fn from(value: (String, u16)) -> Self {
        Self::HostPort {
            host: value.0,
            port: value.1,
        }
    }
}
