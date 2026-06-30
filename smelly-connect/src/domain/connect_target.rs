use std::net::SocketAddr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectTarget {
    host: String,
    port: u16,
}

impl ConnectTarget {
    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl From<(&str, u16)> for ConnectTarget {
    fn from(value: (&str, u16)) -> Self {
        Self {
            host: value.0.to_string(),
            port: value.1,
        }
    }
}

impl From<(String, u16)> for ConnectTarget {
    fn from(value: (String, u16)) -> Self {
        Self {
            host: value.0,
            port: value.1,
        }
    }
}

impl From<SocketAddr> for ConnectTarget {
    fn from(value: SocketAddr) -> Self {
        Self {
            host: value.ip().to_string(),
            port: value.port(),
        }
    }
}
