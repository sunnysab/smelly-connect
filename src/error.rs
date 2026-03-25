use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Bootstrap(BootstrapError),
    Resolve(ResolveError),
    Route(RouteError),
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
    UnexpectedReplyType(u8),
    ReplyTooShort,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    NoRecordFound,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteError {
    TargetNotAllowed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootstrapError {
    NotImplemented,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    ConnectFailed(String),
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
