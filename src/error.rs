use std::fmt::{Display, Formatter};

#[derive(Debug)]
pub enum Error {}

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
