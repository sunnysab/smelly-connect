use std::ffi::c_int;
use std::os::raw::c_long;
use std::sync::OnceLock;

use foreign_types::{ForeignType, ForeignTypeRef};
use openssl::error::ErrorStack;
use openssl::provider::Provider;
use openssl::ssl::{SslConnector, SslConnectorBuilder, SslMethod, SslRef, SslSession, SslVerifyMode, SslVersion};
use openssl_sys as ffi;

pub const HEARTBEAT_EXT_TYPE: u16 = 0x000f;
pub const PROBE_EXT_TYPE: u16 = 0xffa5;
pub const EASYCONNECT_SESSION_ID: &[u8; 32] = b"L3IP\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0";
static LEGACY_PROVIDER: OnceLock<Option<Provider>> = OnceLock::new();
static PROBE_EXT_BYTES: [u8; 1] = [0x42];
static HEARTBEAT_EXT_BYTES: [u8; 1] = [0x01];

unsafe extern "C" {
    fn SSL_SESSION_new() -> *mut ffi::SSL_SESSION;
    fn SSL_SESSION_set1_id(s: *mut ffi::SSL_SESSION, sid: *const u8, sid_len: c_int) -> c_int;
    fn SSL_SESSION_set_protocol_version(s: *mut ffi::SSL_SESSION, version: c_int) -> c_int;
    fn SSL_SESSION_set_time(s: *mut ffi::SSL_SESSION, t: c_long) -> c_long;
    fn SSL_SESSION_set_timeout(s: *mut ffi::SSL_SESSION, t: c_long) -> c_long;
    fn SSL_SESSION_set_cipher(s: *mut ffi::SSL_SESSION, cipher: *const ffi::SSL_CIPHER) -> c_int;
    fn SSL_CIPHER_find(ssl: *mut ffi::SSL, ptr: *const u8) -> *const ffi::SSL_CIPHER;
    fn SSL_CTX_add_client_custom_ext(
        ctx: *mut ffi::SSL_CTX,
        ext_type: u32,
        add_cb: Option<
            unsafe extern "C" fn(
                s: *mut ffi::SSL,
                ext_type: u32,
                out: *mut *const u8,
                outlen: *mut usize,
                al: *mut c_int,
                add_arg: *mut std::ffi::c_void,
            ) -> c_int,
        >,
        free_cb: Option<
            unsafe extern "C" fn(
                s: *mut ffi::SSL,
                ext_type: u32,
                out: *const u8,
                add_arg: *mut std::ffi::c_void,
            ),
        >,
        add_arg: *mut std::ffi::c_void,
        parse_cb: Option<
            unsafe extern "C" fn(
                s: *mut ffi::SSL,
                ext_type: u32,
                input: *const u8,
                inlen: usize,
                al: *mut c_int,
                parse_arg: *mut std::ffi::c_void,
            ) -> c_int,
        >,
        parse_arg: *mut std::ffi::c_void,
    ) -> c_int;
}

pub fn build_easyconnect_connector() -> Result<SslConnector, ErrorStack> {
    let mut builder = SslConnector::builder(SslMethod::tls_client())?;
    configure_easyconnect_context(&mut builder)?;
    Ok(builder.build())
}

pub fn configure_easyconnect_context(builder: &mut SslConnectorBuilder) -> Result<(), ErrorStack> {
    builder.set_verify(SslVerifyMode::NONE);
    unsafe {
        if SSL_CTX_add_client_custom_ext(
            builder.as_ptr(),
            PROBE_EXT_TYPE as u32,
            Some(add_probe_ext),
            Some(free_custom_ext),
            std::ptr::null_mut(),
            Some(parse_custom_ext),
            std::ptr::null_mut(),
        ) != 1
        {
            return Err(ErrorStack::get());
        }
        if SSL_CTX_add_client_custom_ext(
            builder.as_ptr(),
            HEARTBEAT_EXT_TYPE as u32,
            Some(add_heartbeat_ext),
            Some(free_custom_ext),
            std::ptr::null_mut(),
            Some(parse_custom_ext),
            std::ptr::null_mut(),
        ) != 1
        {
            return Err(ErrorStack::get());
        }
    }
    Ok(())
}

pub fn configure_easyconnect_ssl(ssl: &mut SslRef) -> Result<(), ErrorStack> {
    configure_easyconnect_ssl_probe(ssl)?;
    ssl.set_cipher_list("RC4-SHA:@SECLEVEL=0")?;
    Ok(())
}

pub fn configure_easyconnect_ssl_probe(ssl: &mut SslRef) -> Result<(), ErrorStack> {
    let _ = LEGACY_PROVIDER.get_or_init(|| Provider::try_load(None, "legacy", true).ok());
    ssl.set_min_proto_version(Some(SslVersion::TLS1_1))?;
    ssl.set_max_proto_version(Some(SslVersion::TLS1_1))?;
    // This relaxed path is only for local probing. It proves that OpenSSL can emit
    // the fixed session ID and TLS 1.1 ClientHello fields even when the exact
    // legacy cipher or heartbeat extension is unavailable on the host build.
    let _ = ssl.set_cipher_list("DEFAULT:@SECLEVEL=0");
    attach_easyconnect_session_id(ssl)?;
    Ok(())
}

fn attach_easyconnect_session_id(ssl: &mut SslRef) -> Result<(), ErrorStack> {
    unsafe {
        let raw = SSL_SESSION_new();
        let Some(raw) = (!raw.is_null()).then_some(raw) else {
            return Err(ErrorStack::get());
        };
        if SSL_SESSION_set1_id(raw, EASYCONNECT_SESSION_ID.as_ptr(), EASYCONNECT_SESSION_ID.len() as c_int) != 1 {
            ffi::SSL_SESSION_free(raw);
            return Err(ErrorStack::get());
        }
        if SSL_SESSION_set_protocol_version(raw, ffi::TLS1_1_VERSION as c_int) != 1 {
            ffi::SSL_SESSION_free(raw);
            return Err(ErrorStack::get());
        }
        SSL_SESSION_set_time(raw, 0);
        SSL_SESSION_set_timeout(raw, 60);
        let cipher = SSL_CIPHER_find(ssl.as_ptr(), [0x00_u8, 0x2f_u8].as_ptr());
        if cipher.is_null() || SSL_SESSION_set_cipher(raw, cipher) != 1 {
            ffi::SSL_SESSION_free(raw);
            return Err(ErrorStack::get());
        }
        let session = SslSession::from_ptr(raw);
        ssl.set_session(&session)?;
    }
    Ok(())
}

unsafe extern "C" fn add_probe_ext(
    _ssl: *mut ffi::SSL,
    _ext_type: u32,
    out: *mut *const u8,
    outlen: *mut usize,
    _al: *mut c_int,
    _add_arg: *mut std::ffi::c_void,
) -> c_int {
    unsafe {
        *out = PROBE_EXT_BYTES.as_ptr();
        *outlen = PROBE_EXT_BYTES.len();
    }
    1
}

unsafe extern "C" fn add_heartbeat_ext(
    _ssl: *mut ffi::SSL,
    _ext_type: u32,
    out: *mut *const u8,
    outlen: *mut usize,
    _al: *mut c_int,
    _add_arg: *mut std::ffi::c_void,
) -> c_int {
    unsafe {
        *out = HEARTBEAT_EXT_BYTES.as_ptr();
        *outlen = HEARTBEAT_EXT_BYTES.len();
    }
    1
}

unsafe extern "C" fn free_custom_ext(
    _ssl: *mut ffi::SSL,
    _ext_type: u32,
    _out: *const u8,
    _add_arg: *mut std::ffi::c_void,
) {
}

unsafe extern "C" fn parse_custom_ext(
    _ssl: *mut ffi::SSL,
    _ext_type: u32,
    _input: *const u8,
    _inlen: usize,
    _al: *mut c_int,
    _parse_arg: *mut std::ffi::c_void,
) -> c_int {
    1
}
