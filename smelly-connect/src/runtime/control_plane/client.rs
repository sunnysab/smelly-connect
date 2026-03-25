use crate::error::{BootstrapError, Error};

pub(crate) fn build_reqwest_client() -> Result<reqwest::Client, Error> {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|err| Error::Bootstrap(BootstrapError::AuthFlowFailed(err.to_string())))
}
