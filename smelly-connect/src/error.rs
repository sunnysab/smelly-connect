use std::fmt::{Display, Formatter};
use std::io;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    ControlPlane(ControlPlaneError),
    Integration(IntegrationError),
    Proxy(ProxyError),
    Resolve(ResolveError),
    RouteDecision(RouteDecisionError),
    TunnelBootstrap(TunnelBootstrapError),
    Transport(TransportError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    MissingSuccessMarker,
    MissingTwfId,
    InvalidModulusHex,
    InvalidPublicExponent,
    EncryptFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    InvalidSessionIdLength,
    UnexpectedReplyType(u8),
    ReplyTooShort,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    NoRecordFound,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteDecisionError {
    TargetNotAllowed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlPlaneError {
    AuthFlowFailed(String),
    CaptchaRequired,
    NotImplemented,
    ResourceParseFailed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TunnelBootstrapError {
    HandshakeFailed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    ConnectTimedOut,
    ConnectFailed(String),
    ConnectionClosed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyError {
    BindFailed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntegrationError {
    ClientBuildFailed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptchaError {
    message: String,
}

impl CaptchaError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Display for CaptchaError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CaptchaError {}

impl TransportError {
    pub fn from_io(err: io::Error) -> Self {
        match err.kind() {
            io::ErrorKind::TimedOut => Self::ConnectTimedOut,
            io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::UnexpectedEof => Self::ConnectionClosed,
            _ => Self::ConnectFailed(err.to_string()),
        }
    }
}
