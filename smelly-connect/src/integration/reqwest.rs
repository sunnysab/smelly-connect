// Reqwest integration is currently implemented through an internal local proxy
// started from the session. This keeps the public API usable while the direct
// connector path is still under development.
use std::net::SocketAddr;
use std::sync::Arc;

use crate::error::{Error, IntegrationError};
use crate::session::EasyConnectSession;

pub async fn build_client(session: &EasyConnectSession) -> Result<reqwest::Client, Error> {
    let (client, _) = build_client_with_proxy_addr(session).await?;
    Ok(client)
}

#[cfg(any(test, debug_assertions, feature = "test-utils"))]
#[doc(hidden)]
pub async fn build_client_for_test(
    session: &EasyConnectSession,
) -> Result<(reqwest::Client, SocketAddr), Error> {
    build_client_with_proxy_addr(session).await
}

async fn build_client_with_proxy_addr(
    session: &EasyConnectSession,
) -> Result<(reqwest::Client, SocketAddr), Error> {
    let proxy = session.reqwest_proxy().await?;
    let proxy_addr = proxy.local_addr();
    let client = build_client_from_proxy(proxy)?;
    Ok((client, proxy_addr))
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
