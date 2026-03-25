#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetAddr {
    HostPort { host: String, port: u16 },
}

impl From<(&str, u16)> for TargetAddr {
    fn from(value: (&str, u16)) -> Self {
        Self::HostPort {
            host: value.0.to_string(),
            port: value.1,
        }
    }
}
