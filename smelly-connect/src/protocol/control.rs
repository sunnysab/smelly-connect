use std::net::Ipv4Addr;

use crate::error::{AuthError, ProtocolError};

pub fn parse_login_psw_success(body: &str, current_twfid: &str) -> Result<String, AuthError> {
    if !body.contains("<Result>1</Result>") {
        return Err(AuthError::MissingSuccessMarker);
    }

    extract_tag(body, "TwfID")
        .map(ToOwned::to_owned)
        .or_else(|| (!current_twfid.is_empty()).then(|| current_twfid.to_string()))
        .ok_or(AuthError::MissingTwfId)
}

pub fn parse_assigned_ip_reply(reply: &[u8]) -> Result<Ipv4Addr, ProtocolError> {
    if reply.len() < 8 {
        return Err(ProtocolError::ReplyTooShort);
    }
    if reply[0] != 0x00 {
        return Err(ProtocolError::UnexpectedReplyType(reply[0]));
    }

    Ok(Ipv4Addr::new(reply[4], reply[5], reply[6], reply[7]))
}

fn extract_tag<'a>(body: &'a str, tag: &str) -> Option<&'a str> {
    let start_tag = format!("<{tag}>");
    let end_tag = format!("</{tag}>");
    let start = body.find(&start_tag)? + start_tag.len();
    let end = body[start..].find(&end_tag)? + start;
    Some(body[start..end].trim())
}
