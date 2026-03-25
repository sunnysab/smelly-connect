use std::io;
use std::net::SocketAddr;

use crate::proxy::http::ProxyHandle;
use crate::session::EasyConnectSession;

pub async fn start_http_proxy(
    session: EasyConnectSession,
    bind: SocketAddr,
) -> io::Result<ProxyHandle> {
    crate::proxy::http::start_http_proxy(session, bind).await
}
