// Reqwest integration is currently implemented through an internal local proxy
// started from the session. This keeps the public API usable while the direct
// connector path is still under development.
use std::sync::Arc;

use crate::error::{Error, IntegrationError};
use crate::session::EasyConnectSession;

pub async fn build_client(session: &EasyConnectSession) -> Result<reqwest::Client, Error> {
    let proxy = session.reqwest_proxy().await?;
    build_client_from_proxy(proxy)
}

fn build_client_from_proxy(
    proxy: Arc<crate::session::SessionReqwestProxy>,
) -> Result<reqwest::Client, Error> {
    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::custom({
            let proxy = Arc::clone(&proxy);
            move |_| Some(proxy.proxy_url().clone())
        }))
        .build()
        .map_err(|err| Error::Integration(IntegrationError::ClientBuildFailed(err.to_string())))?;
    Ok(client)
}
