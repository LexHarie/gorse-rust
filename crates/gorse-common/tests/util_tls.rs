// Checks common/util/tls.go and master/rpc_test.go (TestSSL) @ add21b5

//! `util::tls` against what Go's `util.NewServerCreds` and
//! `util.NewClientCreds` do with the same files, written by
//! `contracts/tools/tlsgolden` to `tests/data/tls_golden.json` (the files are
//! in `tests/data/tls/`): for each pair of server and client settings, whether
//! each configuration builds and, when both do, whether a handshake and a
//! ping and pong go through, which side rejects the other, and the ALPN
//! protocol. The cases include Go's TestSSL (success, plaintext client,
//! certificate from another CA).

#[path = "support/tls_peer.rs"]
mod tls_peer;

use std::net::TcpListener;
use std::thread;

use gorse_common::util::tls::{self, TlsConfig};
use serde_json::Value;
use tls_peer::{Mode, Outcome};

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/tls");

fn golden() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/tls_golden.json");
    let text = std::fs::read_to_string(path).expect("read tls_golden.json");
    serde_json::from_str(&text).expect("parse tls_golden.json")
}

fn s(v: &Value) -> &str {
    v.as_str().expect("string")
}

/// One side's files, as the Go tool's `config`.
fn files(side: &Value) -> TlsConfig {
    let path = |key: &str| format!("{DIR}/{}", s(&side[key]));
    TlsConfig {
        ssl_ca: path("ca"),
        ssl_cert: path("cert"),
        ssl_key: path("key"),
    }
}

/// The verdict on one case, in the golden's terms.
#[derive(Debug, PartialEq, Eq)]
struct Verdict {
    server_config_ok: bool,
    client_config_ok: bool,
    established: bool,
    rejected_by: String,
    alpn: String,
}

fn verdict(server: Outcome, client: Outcome) -> Verdict {
    let established = server.exchanged && client.exchanged;
    let rejected_by = match (server.local, client.local) {
        (true, true) => "both",
        (true, false) => "server",
        (false, true) => "client",
        (false, false) => "",
    };
    Verdict {
        server_config_ok: true,
        client_config_ok: true,
        established,
        rejected_by: rejected_by.to_owned(),
        alpn: if established {
            client.alpn
        } else {
            String::new()
        },
    }
}

fn run(case: &Value) -> Verdict {
    let mode = Mode::parse(s(&case["client_mode"]));
    let server = tls_peer::server_config(&files(&case["server"]));
    let client = tls_peer::client_config(mode, &files(&case["client"]));
    let (server, client) = match (server, client) {
        (Ok(server), Ok(client)) => (server, client),
        (server, client) => {
            return Verdict {
                server_config_ok: server.is_ok(),
                client_config_ok: client.is_ok(),
                established: false,
                rejected_by: String::new(),
                alpn: String::new(),
            };
        }
    };
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("address").to_string();
    let served = thread::spawn(move || tls_peer::serve(&listener, server));
    let client = tls_peer::connect(&addr, client);
    let server = served.join().expect("server thread");
    verdict(server, client)
}

#[test]
fn util_tls_matches_go_on_every_case() {
    let golden = golden();
    let cases = golden["cases"].as_array().expect("cases");
    assert_eq!(cases.len(), 62);
    let mut differ = Vec::new();
    for case in cases {
        let want = Verdict {
            server_config_ok: s(&case["server_config_error"]).is_empty(),
            client_config_ok: s(&case["client_config_error"]).is_empty(),
            established: case["established"].as_bool().expect("established"),
            rejected_by: s(&case["rejected_by"]).to_owned(),
            alpn: s(&case["alpn"]).to_owned(),
        };
        let got = run(case);
        if got != want {
            differ.push(format!("{}: Go {want:?}, Rust {got:?}", s(&case["name"])));
        }
    }
    assert!(
        differ.is_empty(),
        "{} cases differ from Go:\n{}",
        differ.len(),
        differ.join("\n")
    );
}

/// Gorse's TestSSL, step by step, with the outcome of each side.
#[test]
fn util_tls_test_ssl() {
    let node = TlsConfig {
        ssl_ca: format!("{DIR}/ca.pem"),
        ssl_cert: format!("{DIR}/node.pem"),
        ssl_key: format!("{DIR}/node.key"),
    };
    let handshake = |client: Option<std::sync::Arc<rustls::ClientConfig>>| {
        let server = tls_peer::server_config(&node).expect("server config");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("address").to_string();
        let served = thread::spawn(move || tls_peer::serve(&listener, server));
        let client = tls_peer::connect(&addr, client);
        (served.join().expect("server thread"), client)
    };

    // success
    let client = tls_peer::client_config(Mode::Creds, &node).expect("client config");
    let (server, client) = handshake(client);
    assert!(
        server.exchanged && client.exchanged,
        "{server:?} {client:?}"
    );
    assert_eq!(client.alpn, "h2");

    // insecure
    let (server, client) = handshake(None);
    assert!(!server.exchanged && server.local, "{server:?}");
    assert!(!client.exchanged, "{client:?}");

    // certificate mismatch
    let node2 = TlsConfig {
        ssl_ca: format!("{DIR}/ca2.pem"),
        ssl_cert: format!("{DIR}/node2.pem"),
        ssl_key: format!("{DIR}/node2.key"),
    };
    let client = tls_peer::client_config(Mode::Creds, &node2).expect("client config");
    let (server, client) = handshake(client);
    assert!(!client.exchanged && client.local, "{client:?}");
    // Both CAs have the same subject, so the signature check fails (Go: "x509:
    // certificate signed by unknown authority (possibly because of ...
    // ECDSA verification failure ...)").
    assert!(
        client
            .handshake_error
            .starts_with("invalid peer certificate"),
        "{client:?}"
    );
    assert!(!server.exchanged && !server.local, "{server:?}");
}

/// The configuration errors name what went wrong, as Go's do.
#[test]
fn util_tls_config_errors() {
    let with = |ca: &str, cert: &str, key: &str| TlsConfig {
        ssl_ca: format!("{DIR}/{ca}"),
        ssl_cert: format!("{DIR}/{cert}"),
        ssl_key: format!("{DIR}/{key}"),
    };
    let err = |o: &TlsConfig| tls::new_server_config(o).expect_err("error").to_string();
    let missing = err(&with("missing.pem", "node.pem", "node.key"));
    assert!(
        missing.starts_with(&format!("open {DIR}/missing.pem: ")),
        "{missing}"
    );
    assert_eq!(
        err(&with("ca-none.pem", "node.pem", "node.key")),
        "failed to append certificate"
    );
    assert_eq!(
        err(&with("ca-corrupt.pem", "node.pem", "node.key")),
        "failed to append certificate"
    );
    assert_eq!(
        err(&with("ca.pem", "cert-none.pem", "node.key")),
        "tls: failed to find any PEM data in certificate input"
    );
    assert_eq!(
        err(&with("ca.pem", "node.pem", "key-none.key")),
        "tls: failed to find any PEM data in key input"
    );
    let mismatch = err(&with("ca.pem", "node.pem", "p256-sec1.key"));
    assert!(mismatch.starts_with("tls: "), "{mismatch}");
    let client = tls::new_client_config(&with("ca.pem", "node.pem", "p256-sec1.key"));
    assert!(client.is_err());
}

/// gRPC-Go clients send `host:port` as the TLS server name; the server
/// ignores a name that is not a DNS name instead of failing the handshake
/// (`third_party/rustls/GORSE.md`). The client hello here is a real one with
/// `localhost` overwritten by a name of the same length.
#[test]
fn util_tls_server_ignores_a_host_and_port_server_name() {
    use std::sync::Arc;

    use rustls::pki_types::ServerName;
    use rustls::{ClientConnection, ServerConnection};

    let node = TlsConfig {
        ssl_ca: format!("{DIR}/ca.pem"),
        ssl_cert: format!("{DIR}/node.pem"),
        ssl_key: format!("{DIR}/node.key"),
    };
    let client_config = Arc::new(tls::new_client_config(&node).expect("client config"));
    let name = ServerName::try_from("localhost").expect("name");
    let mut client = ClientConnection::new(client_config, name).expect("client");
    let mut hello = Vec::new();
    while client.wants_write() {
        client.write_tls(&mut hello).expect("client hello");
    }
    let at = hello
        .windows(9)
        .position(|w| w == b"localhost")
        .expect("server name in the hello");
    hello[at..at + 9].copy_from_slice(b"local:443");

    let server_config = Arc::new(tls::new_server_config(&node).expect("server config"));
    let mut server = ServerConnection::new(server_config).expect("server");
    let mut input = hello.as_slice();
    while !input.is_empty() {
        server.read_tls(&mut input).expect("read");
    }
    server
        .process_new_packets()
        .expect("the server accepts the client hello");
    assert!(server.wants_write(), "the server answers with its hello");
    assert_eq!(server.server_name(), None);
}
