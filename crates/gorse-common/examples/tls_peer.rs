//! The Rust half of `contracts/tools/tlsgolden -interop`, which runs Gorse's
//! TLS cases between Go and Rust peers:
//!
//! ```text
//! tls_peer check-server|check-client|server|client <addr> <mode> <ca> <cert> <key>
//! ```
//!
//! `check-*` builds the configuration and prints the outcome. `server` binds
//! `<addr>`, prints `{"addr": ...}`, serves one connection and prints the
//! outcome. `client` connects to `<addr>` in `<mode>` (`creds`, `no-cert` or
//! `plaintext`) and prints the outcome. Outcomes are one JSON line each.

#[path = "../tests/support/tls_peer.rs"]
mod tls_peer;

use std::net::TcpListener;

use gorse_common::util::tls::TlsConfig;

use tls_peer::{Mode, Outcome};

fn print(o: &Outcome) {
    println!("{}", serde_json::to_string(o).expect("outcome JSON"));
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [role, addr, mode, ca, cert, key] = <[String; 6]>::try_from(args).unwrap_or_else(|args| {
        eprintln!("usage: tls_peer check-server|check-client|server|client <addr> <mode> <ca> <cert> <key>; got {args:?}");
        std::process::exit(2)
    });
    let files = TlsConfig {
        ssl_ca: ca,
        ssl_cert: cert,
        ssl_key: key,
    };
    let mode = Mode::parse(&mode);
    match role.as_str() {
        "check-server" => print(&tls_peer::server_config(&files).err().unwrap_or_default()),
        "check-client" => print(
            &tls_peer::client_config(mode, &files)
                .err()
                .unwrap_or_default(),
        ),
        "server" => {
            let config = tls_peer::server_config(&files).expect("server configuration");
            let listener = TcpListener::bind(&addr).expect("bind");
            let local = listener.local_addr().expect("address");
            println!("{}", serde_json::json!({ "addr": local.to_string() }));
            print(&tls_peer::serve(&listener, config));
        }
        "client" => {
            let config = tls_peer::client_config(mode, &files).expect("client configuration");
            print(&tls_peer::connect(&addr, config));
        }
        _ => {
            eprintln!("unknown role {role:?}");
            std::process::exit(2)
        }
    }
}
