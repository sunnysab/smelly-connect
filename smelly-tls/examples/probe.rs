use std::env;
use std::net::ToSocketAddrs;

use smelly_tls::{
    ClientHelloConfig, TLS_RSA_WITH_AES_128_CBC_SHA, TLS_RSA_WITH_RC4_128_SHA,
    complete_minimal_handshake, connect_and_read_server_flight,
};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let host = env::args()
        .nth(1)
        .expect("usage: probe <host:port> <rc4|aes128-sha> [comp]");
    let mode = env::args()
        .nth(2)
        .expect("usage: probe <host:port> <rc4|aes128-sha> [comp]");
    let compression = env::args().nth(3);
    let cipher_suite = match mode.as_str() {
        "rc4" => TLS_RSA_WITH_RC4_128_SHA,
        "aes128-sha" => TLS_RSA_WITH_AES_128_CBC_SHA,
        other => panic!("unknown mode: {other}"),
    };

    let addr = host
        .to_socket_addrs()
        .unwrap()
        .next()
        .expect("resolved addr");
    let config = ClientHelloConfig::new(
        [0x41; 32],
        *b"L3IP\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
    )
    .with_cipher_suite(cipher_suite);
    let config = if compression.as_deref() == Some("comp") {
        config.with_compression_methods(vec![1, 0])
    } else {
        config
    };

    match complete_minimal_handshake(addr, &config).await {
        Ok(result) => {
            println!("handshake ok");
            println!("server cipher: 0x{:04x}", result.server_hello.cipher_suite);
            println!(
                "server session id: {}",
                hex::encode(result.server_hello.session_id)
            );
        }
        Err(err) => {
            eprintln!("handshake failed: {err}");
            match connect_and_read_server_flight(addr, &config).await {
                Ok(flight) => {
                    eprintln!("server flight parsed");
                    eprintln!("server cipher: 0x{:04x}", flight.server_hello.cipher_suite);
                    eprintln!(
                        "server session id: {}",
                        hex::encode(flight.server_hello.session_id)
                    );
                    eprintln!("cert count: {}", flight.certificate_chain.len());
                    eprintln!("server hello done: {}", flight.server_hello_done);
                    if let Err(err) = smelly_tls::probe_handshake_steps(addr, &config).await {
                        eprintln!("step probe failed: {err}");
                    }
                }
                Err(err) => {
                    eprintln!("server flight probe failed: {err}");
                }
            }
            std::process::exit(1);
        }
    }
}
