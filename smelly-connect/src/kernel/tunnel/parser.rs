use std::net::Ipv4Addr;

use crate::error::ProtocolError;

pub fn parse_assigned_ip_reply(reply: &[u8]) -> Result<Ipv4Addr, ProtocolError> {
    if reply.len() < 8 {
        return Err(ProtocolError::ReplyTooShort);
    }
    if reply[0] != 0x00 {
        return Err(ProtocolError::UnexpectedReplyType(reply[0]));
    }

    Ok(Ipv4Addr::new(reply[4], reply[5], reply[6], reply[7]))
}
