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

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ControlPlane(e) => write!(f, "control plane: {e}"),
            Self::Integration(e) => write!(f, "integration: {e}"),
            Self::Proxy(e) => write!(f, "proxy: {e}"),
            Self::Resolve(e) => write!(f, "resolve: {e}"),
            Self::RouteDecision(e) => write!(f, "route decision: {e}"),
            Self::TunnelBootstrap(e) => write!(f, "tunnel bootstrap: {e}"),
            Self::Transport(e) => write!(f, "transport: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ControlPlane(e) => Some(e),
            Self::Integration(e) => Some(e),
            Self::Proxy(e) => Some(e),
            Self::Resolve(e) => Some(e),
            Self::RouteDecision(e) => Some(e),
            Self::TunnelBootstrap(e) => Some(e),
            Self::Transport(e) => Some(e),
        }
    }
}

impl Error {
    pub fn is_permanent_auth_failure(&self) -> bool {
        matches!(
            self,
            Self::ControlPlane(error) if error.is_permanent_auth_failure()
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    MissingSuccessMarker,
    MissingTwfId,
    InvalidModulusHex,
    InvalidPublicExponent,
    EncryptFailed,
}

impl Display for AuthError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::MissingSuccessMarker => "MissingSuccessMarker",
            Self::MissingTwfId => "MissingTwfId",
            Self::InvalidModulusHex => "InvalidModulusHex",
            Self::InvalidPublicExponent => "InvalidPublicExponent",
            Self::EncryptFailed => "EncryptFailed",
        };
        f.write_str(label)
    }
}

impl std::error::Error for AuthError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    InvalidSessionIdLength,
    UnexpectedReplyType(u8),
    ReplyTooShort,
}

impl Display for ProtocolError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSessionIdLength => f.write_str("invalid session id length"),
            Self::UnexpectedReplyType(ty) => write!(f, "unexpected reply type: 0x{ty:02x}"),
            Self::ReplyTooShort => f.write_str("reply too short"),
        }
    }
}

impl std::error::Error for ProtocolError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    NoRecordFound,
}

impl Display for ResolveError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoRecordFound => f.write_str("no DNS record found"),
        }
    }
}

impl std::error::Error for ResolveError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteDecisionError {
    TargetNotAllowed,
}

impl Display for RouteDecisionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TargetNotAllowed => f.write_str("target not allowed by route policy"),
        }
    }
}

impl std::error::Error for RouteDecisionError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlPlaneError {
    AuthFlowFailed(String),
    PermanentAuthFailure(AuthError),
    CaptchaRequired,
    ResourceParseFailed(String),
}

impl Display for ControlPlaneError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AuthFlowFailed(msg) => write!(f, "auth flow failed: {msg}"),
            Self::PermanentAuthFailure(e) => write!(f, "permanent auth failure: {e}"),
            Self::CaptchaRequired => f.write_str("captcha required"),
            Self::ResourceParseFailed(msg) => write!(f, "resource parse failed: {msg}"),
        }
    }
}

impl std::error::Error for ControlPlaneError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::PermanentAuthFailure(e) => Some(e),
            _ => None,
        }
    }
}

impl ControlPlaneError {
    pub fn from_auth_error(error: AuthError) -> Self {
        if matches!(error, AuthError::MissingSuccessMarker) {
            Self::PermanentAuthFailure(error)
        } else {
            Self::AuthFlowFailed(error.to_string())
        }
    }

    pub fn is_permanent_auth_failure(&self) -> bool {
        matches!(self, Self::PermanentAuthFailure(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TunnelBootstrapError {
    HandshakeFailed(String),
}

impl Display for TunnelBootstrapError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HandshakeFailed(msg) => write!(f, "tunnel handshake failed: {msg}"),
        }
    }
}

impl std::error::Error for TunnelBootstrapError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    ConnectTimedOut,
    ConnectFailed(String),
    ConnectionClosed,
}

impl Display for TransportError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConnectTimedOut => f.write_str("connect timed out"),
            Self::ConnectFailed(msg) => write!(f, "connect failed: {msg}"),
            Self::ConnectionClosed => f.write_str("connection closed"),
        }
    }
}

impl std::error::Error for TransportError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyError {
    BindFailed(String),
}

impl Display for ProxyError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BindFailed(msg) => write!(f, "proxy bind failed: {msg}"),
        }
    }
}

impl std::error::Error for ProxyError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntegrationError {
    ClientBuildFailed(String),
}

impl Display for IntegrationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ClientBuildFailed(msg) => write!(f, "integration client build failed: {msg}"),
        }
    }
}

impl std::error::Error for IntegrationError {}

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

#[cfg(test)]
mod tests {
    use super::{AuthError, ControlPlaneError, Error};

    #[test]
    fn missing_success_marker_is_classified_as_permanent_auth_failure() {
        let error = Error::ControlPlane(ControlPlaneError::from_auth_error(
            AuthError::MissingSuccessMarker,
        ));
        assert!(error.is_permanent_auth_failure());
        assert!(matches!(
            error,
            Error::ControlPlane(ControlPlaneError::PermanentAuthFailure(
                AuthError::MissingSuccessMarker
            ))
        ));
    }

    #[test]
    fn other_auth_errors_remain_generic_auth_flow_failures() {
        let error = ControlPlaneError::from_auth_error(AuthError::MissingTwfId);
        assert_eq!(
            error,
            ControlPlaneError::AuthFlowFailed("MissingTwfId".to_string())
        );
        assert!(!error.is_permanent_auth_failure());
    }
}
