use std::collections::HashMap;

use reqwest::header::{CONTENT_TYPE, COOKIE, USER_AGENT};

use crate::auth::{encrypt_password, parse_login_auth};
use crate::config::EasyConnectConfig;
use crate::error::{ControlPlaneError, Error};
use crate::resource::parse_resources;

use super::client::build_reqwest_client;
use super::types::ControlPlaneState;

pub async fn run_control_plane(config: &EasyConnectConfig) -> Result<ControlPlaneState, Error> {
    let client = build_reqwest_client()?;
    let base_url = config.control_base_url();

    let login_auth_body = client
        .get(format!("{base_url}/por/login_auth.csp?apiversion=1"))
        .send()
        .await
        .map_err(|err| Error::ControlPlane(ControlPlaneError::AuthFlowFailed(err.to_string())))?
        .text()
        .await
        .map_err(|err| Error::ControlPlane(ControlPlaneError::AuthFlowFailed(err.to_string())))?;

    let parsed = parse_login_auth(&login_auth_body).map_err(|err| {
        Error::ControlPlane(ControlPlaneError::AuthFlowFailed(format!("{err:?}")))
    })?;

    let mut rand_code = String::new();
    if parsed.requires_captcha {
        let captcha_handler = config
            .captcha_handler
            .clone()
            .ok_or(Error::ControlPlane(ControlPlaneError::CaptchaRequired))?;
        let response = client
            .get(format!("{base_url}/por/rand_code.csp?apiversion=1"))
            .header(COOKIE, format!("TWFID={}", parsed.twfid))
            .header(USER_AGENT, "EasyConnect_windows")
            .send()
            .await
            .map_err(|err| {
                Error::ControlPlane(ControlPlaneError::AuthFlowFailed(err.to_string()))
            })?;
        let mime_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        let bytes = response.bytes().await.map_err(|err| {
            Error::ControlPlane(ControlPlaneError::AuthFlowFailed(err.to_string()))
        })?;
        rand_code = captcha_handler
            .solve(bytes.to_vec(), mime_type)
            .await
            .map_err(|err| {
                Error::ControlPlane(ControlPlaneError::AuthFlowFailed(err.to_string()))
            })?;
    }

    let encrypted_password = encrypt_password(
        &config.password,
        parsed.csrf_rand_code.as_deref(),
        &parsed.rsa_key_hex,
        parsed.rsa_exp,
    )
    .map_err(|err| Error::ControlPlane(ControlPlaneError::AuthFlowFailed(format!("{err:?}"))))?;

    let mut form = HashMap::new();
    form.insert("svpn_rand_code", rand_code);
    form.insert("mitm", String::new());
    form.insert(
        "svpn_req_randcode",
        parsed.csrf_rand_code.clone().unwrap_or_default(),
    );
    form.insert("svpn_name", config.username.clone());
    form.insert("svpn_password", encrypted_password);

    let login_psw_body = client
        .post(format!(
            "{base_url}/por/login_psw.csp?anti_replay=1&encrypt=1&type=cs"
        ))
        .header(COOKIE, format!("TWFID={}", parsed.twfid))
        .header(USER_AGENT, "EasyConnect_windows")
        .form(&form)
        .send()
        .await
        .map_err(|err| Error::ControlPlane(ControlPlaneError::AuthFlowFailed(err.to_string())))?
        .text()
        .await
        .map_err(|err| Error::ControlPlane(ControlPlaneError::AuthFlowFailed(err.to_string())))?;

    let authorized_twfid = crate::protocol::parse_login_psw_success(&login_psw_body, &parsed.twfid)
        .map_err(|err| {
            Error::ControlPlane(ControlPlaneError::AuthFlowFailed(format!("{err:?}")))
        })?;

    let resource_body = client
        .get(format!("{base_url}/por/rclist.csp"))
        .header(COOKIE, format!("TWFID={authorized_twfid}"))
        .send()
        .await
        .map_err(|err| Error::ControlPlane(ControlPlaneError::AuthFlowFailed(err.to_string())))?
        .text()
        .await
        .map_err(|err| Error::ControlPlane(ControlPlaneError::AuthFlowFailed(err.to_string())))?;

    let resources = parse_resources(&resource_body).map_err(|err| {
        Error::ControlPlane(ControlPlaneError::ResourceParseFailed(err.to_string()))
    })?;

    Ok(ControlPlaneState {
        authorized_twfid,
        legacy_cipher_hint: parsed.legacy_cipher_hint,
        resources,
        token: None,
    })
}
