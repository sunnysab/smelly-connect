use crate::error::{ControlPlaneError, Error};

pub(crate) fn build_reqwest_client() -> Result<reqwest::Client, Error> {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|err| Error::ControlPlane(ControlPlaneError::AuthFlowFailed(err.to_string())))
}
