// Reqwest integration is currently implemented through an internal local proxy
// started from the session. This keeps the public API usable while the direct
// connector path is still under development.
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use crate::error::{Error, IntegrationError};
use crate::proxy::http::ProxyHandle;
use crate::session::EasyConnectSession;

pub async fn build_client(session: &EasyConnectSession) -> Result<reqwest::Client, Error> {
    let (client, _) = build_client_with_proxy_addr(session).await?;
    Ok(client)
}

#[doc(hidden)]
pub async fn build_client_for_test(
    session: &EasyConnectSession,
) -> Result<(reqwest::Client, SocketAddr), Error> {
    build_client_with_proxy_addr(session).await
}

async fn build_client_with_proxy_addr(
    session: &EasyConnectSession,
) -> Result<(reqwest::Client, SocketAddr), Error> {
    let handle = session
        .start_http_proxy(SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 0)))
        .await?;
    let proxy_addr = handle.local_addr();
    let client = build_client_from_handle(handle)?;
    Ok((client, proxy_addr))
}

fn build_client_from_handle(handle: ProxyHandle) -> Result<reqwest::Client, Error> {
    let guard = Arc::new(ReqwestProxyGuard::new(handle)?);
    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::custom({
            let guard = Arc::clone(&guard);
            move |_| Some(guard.proxy_url.clone())
        }))
        .build()
        .map_err(|err| Error::Integration(IntegrationError::ClientBuildFailed(err.to_string())))?;
    Ok(client)
}

struct ReqwestProxyGuard {
    proxy_url: reqwest::Url,
    handle: Mutex<Option<ProxyHandle>>,
}

impl ReqwestProxyGuard {
    fn new(handle: ProxyHandle) -> Result<Self, Error> {
        let proxy_url = reqwest::Url::parse(&format!("http://{}", handle.local_addr()))
            .map_err(|err| Error::Integration(IntegrationError::ClientBuildFailed(err.to_string())))?;
        Ok(Self {
            proxy_url,
            handle: Mutex::new(Some(handle)),
        })
    }
}

impl Drop for ReqwestProxyGuard {
    fn drop(&mut self) {
        let Some(handle) = self.handle.get_mut().ok().and_then(Option::take) else {
            return;
        };

        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = handle.shutdown().await;
            });
            return;
        }

        std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            runtime.block_on(async move {
                let _ = handle.shutdown().await;
            });
        });
    }
}
