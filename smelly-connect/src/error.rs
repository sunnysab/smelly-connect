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
    PermanentAuthFailure(AuthError),
    CaptchaRequired,
    NotImplemented,
    ResourceParseFailed(String),
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
