use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::error::CaptchaError;

type CaptchaFuture = Pin<Box<dyn Future<Output = Result<String, CaptchaError>> + Send + 'static>>;
type CaptchaFn = dyn Fn(Vec<u8>, Option<String>) -> CaptchaFuture + Send + Sync + 'static;

#[derive(Clone)]
pub struct CaptchaHandler {
    inner: Arc<CaptchaFn>,
}

impl CaptchaHandler {
    pub fn from_async<F, Fut>(callback: F) -> Self
    where
        F: Fn(Vec<u8>, Option<String>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<String, CaptchaError>> + Send + 'static,
    {
        Self {
            inner: Arc::new(move |bytes, mime| Box::pin(callback(bytes, mime))),
        }
    }

    pub async fn solve(
        &self,
        image: Vec<u8>,
        mime_type: Option<String>,
    ) -> Result<String, CaptchaError> {
        (self.inner)(image, mime_type).await
    }
}
