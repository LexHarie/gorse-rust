//! One TLS peer of the Go comparisons for `util::tls`: the test in
//! `tests/util_tls.rs` runs both peers in process, and the `tls_peer` example,
//! which `contracts/tools/tlsgolden -interop` runs against Go, runs one. Both
//! use this code, which does what the Go tool's `serve` and `dial` do: a
//! handshake over TCP, then "ping\n" from the client and "pong\n" back.

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::ops::DerefMut;
use std::sync::Arc;
use std::time::Duration;

use gorse_common::util::tls::{self, TlsConfig};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{
    ClientConfig, ClientConnection, ConnectionCommon, RootCertStore, ServerConfig,
    ServerConnection, SideData,
};
use serde::Serialize;

const DEADLINE: Duration = Duration::from_secs(10);

/// What one peer saw, as the Go tool's `Outcome`.
#[derive(Debug, Default, Serialize)]
pub struct Outcome {
    pub config_error: String,
    pub handshake_error: String,
    /// This side failed of its own accord, not because the peer sent an
    /// alert or closed the connection.
    pub local: bool,
    pub exchanged: bool,
    pub alpn: String,
}

/// How the client connects (the Go tool's client modes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// With [`tls::new_client_config`].
    Creds,
    /// Verifying the server against the CA file, presenting no certificate.
    NoCert,
    /// Sending gRPC's HTTP/2 preface without TLS.
    Plaintext,
}

impl Mode {
    pub fn parse(s: &str) -> Mode {
        match s {
            "creds" => Mode::Creds,
            "no-cert" => Mode::NoCert,
            "plaintext" => Mode::Plaintext,
            _ => panic!("unknown client mode {s:?}"),
        }
    }
}

/// The server's configuration, or the error as an outcome.
pub fn server_config(o: &TlsConfig) -> Result<Arc<ServerConfig>, Outcome> {
    tls::new_server_config(o)
        .map(Arc::new)
        .map_err(|e| Outcome {
            config_error: e.to_string(),
            ..Outcome::default()
        })
}

/// The client's configuration (none in plaintext), or the error as an
/// outcome.
pub fn client_config(mode: Mode, o: &TlsConfig) -> Result<Option<Arc<ClientConfig>>, Outcome> {
    let config = match mode {
        Mode::Creds => tls::new_client_config(o).map_err(|e| e.to_string()),
        Mode::NoCert => no_cert_config(&o.ssl_ca),
        Mode::Plaintext => return Ok(None),
    };
    config
        .map(|c| Some(Arc::new(c)))
        .map_err(|config_error| Outcome {
            config_error,
            ..Outcome::default()
        })
}

/// A client that checks the server, host name included, against the CA file
/// and presents no certificate: Go's `tls.Config{RootCAs: ca}`.
fn no_cert_config(ca: &str) -> Result<ClientConfig, String> {
    let pem = std::fs::read(ca).map_err(|e| e.to_string())?;
    let mut roots = RootCertStore::empty();
    roots.add_parsable_certificates(CertificateDer::pem_slice_iter(&pem).filter_map(Result::ok));
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"h2".to_vec()];
    Ok(config)
}

/// Whether an I/O error from `rustls` is this side's own failure: a TLS error
/// other than an alert from the peer. Plain I/O errors (end of file, reset)
/// come from the peer closing.
fn local(e: &io::Error) -> bool {
    match e
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<rustls::Error>())
    {
        Some(rustls::Error::AlertReceived(_)) => false,
        Some(_) => true,
        None => false,
    }
}

fn fail(mut o: Outcome, e: &io::Error) -> Outcome {
    o.handshake_error = e.to_string();
    o.local = local(e);
    o
}

fn timeouts(sock: &TcpStream) {
    sock.set_read_timeout(Some(DEADLINE)).expect("read timeout");
    sock.set_write_timeout(Some(DEADLINE))
        .expect("write timeout");
}

/// Runs the handshake, the ping and pong, and the close on `conn`.
fn talk<C, S>(mut conn: C, mut sock: TcpStream, client: bool) -> Outcome
where
    C: DerefMut<Target = ConnectionCommon<S>>,
    S: SideData,
{
    let mut o = Outcome::default();
    while conn.is_handshaking() {
        if let Err(e) = conn.complete_io(&mut sock) {
            return fail(o, &e);
        }
    }
    o.alpn = conn
        .alpn_protocol()
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .unwrap_or_default();
    let (send, expect): (&[u8], &[u8]) = if client {
        (b"ping\n", b"pong\n")
    } else {
        (b"", b"ping\n")
    };
    let mut stream = rustls::Stream::new(&mut conn, &mut sock);
    if !send.is_empty()
        && let Err(e) = stream.write_all(send).and_then(|()| stream.flush())
    {
        return fail(o, &e);
    }
    let mut buf = [0; 5];
    if let Err(e) = stream.read_exact(&mut buf) {
        return fail(o, &e);
    }
    if buf != expect {
        o.handshake_error = format!("unexpected {:?}", String::from_utf8_lossy(&buf));
        return o;
    }
    if !client && let Err(e) = stream.write_all(b"pong\n").and_then(|()| stream.flush()) {
        return fail(o, &e);
    }
    o.exchanged = true;
    conn.send_close_notify();
    while conn.wants_write() {
        if conn.write_tls(&mut sock).is_err() {
            break;
        }
    }
    o
}

/// Accepts one connection on `listener` and serves it.
pub fn serve(listener: &TcpListener, config: Arc<ServerConfig>) -> Outcome {
    let (sock, _) = listener.accept().expect("accept");
    timeouts(&sock);
    let conn = ServerConnection::new(config).expect("server connection");
    talk(conn, sock, false)
}

/// Connects to `addr` as the client (`config` is `None` in plaintext).
pub fn connect(addr: &str, config: Option<Arc<ClientConfig>>) -> Outcome {
    let mut sock = TcpStream::connect(addr).expect("connect");
    timeouts(&sock);
    let Some(config) = config else {
        let mut o = Outcome::default();
        let mut rest = Vec::new();
        let result = sock
            .write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n")
            .and_then(|()| sock.read_to_end(&mut rest));
        o.handshake_error = match result {
            Ok(_) => "EOF".to_owned(),
            Err(e) => e.to_string(),
        };
        return o;
    };
    let name = ServerName::try_from("localhost").expect("server name");
    let conn = ClientConnection::new(config, name).expect("client connection");
    talk(conn, sock, true)
}
