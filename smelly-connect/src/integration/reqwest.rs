// Reqwest integration is currently implemented through an internal local proxy
// started from the session. This keeps the public API usable while the direct
// connector path is still under development.
use crate::error::{Error, IntegrationError};
use crate::session::EasyConnectSession;

pub async fn build_client(session: &EasyConnectSession) -> Result<reqwest::Client, Error> {
    let handle = session
        .start_http_proxy("127.0.0.1:0".parse().unwrap())
        .await?;
    let client = reqwest::Client::builder()
        .proxy(
            reqwest::Proxy::all(format!("http://{}", handle.local_addr())).map_err(|err| {
                Error::Integration(IntegrationError::ClientBuildFailed(err.to_string()))
            })?,
        )
        .build()
        .map_err(|err| Error::Integration(IntegrationError::ClientBuildFailed(err.to_string())))?;
    std::mem::forget(handle);
    Ok(client)
}
