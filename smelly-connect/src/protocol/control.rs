use std::net::Ipv4Addr;

use crate::error::{AuthError, ProtocolError};

pub fn parse_login_psw_success(body: &str, current_twfid: &str) -> Result<String, AuthError> {
    crate::kernel::control::parse_login_success(body, current_twfid).map_err(|err| match err {
        crate::kernel::control::ControlParseError::MissingSuccessMarker => {
            AuthError::MissingSuccessMarker
        }
        crate::kernel::control::ControlParseError::MissingTwfId => AuthError::MissingTwfId,
        crate::kernel::control::ControlParseError::MissingTag(_)
        | crate::kernel::control::ControlParseError::InvalidRsaExponent => {
            unreachable!("login success parser returned an unexpected control parse error")
        }
    })
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
