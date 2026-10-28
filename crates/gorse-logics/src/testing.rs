// Ports: the parts of net/http/httptest that logics' tests use. Go's
// httptest.NewServer answers every request through a handler and lets the test
// read what it received; this server does the same over a local port.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver};
use std::thread;

/// A request the test server received.
#[derive(Debug, Clone)]
pub(crate) struct Request {
    /// The method, like `GET`.
    pub method: String,
    /// The request target: the path and the query.
    pub target: String,
    /// The header fields, with their names as sent.
    pub headers: Vec<(String, String)>,
    /// The body, as sent.
    pub body: Vec<u8>,
}

impl Request {
    /// The value of the first header with `name`, compared without case.
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// A reply the test server sends.
#[derive(Debug, Clone)]
pub(crate) struct Reply {
    /// The status code.
    pub status: u16,
    /// The reason phrase after the code.
    pub reason: &'static str,
    /// Extra header fields; `Content-Length`, `Date` and `Connection` are added.
    pub headers: Vec<(String, String)>,
    /// The body.
    pub body: String,
}

impl Reply {
    /// A 200 OK reply with `body`.
    pub(crate) fn ok(body: impl Into<String>) -> Reply {
        Reply {
            status: 200,
            reason: "OK",
            headers: Vec::new(),
            body: body.into(),
        }
    }

    /// A 404 Not Found reply, as `http.NotFound` writes it.
    pub(crate) fn not_found() -> Reply {
        Reply {
            status: 404,
            reason: "Not Found",
            headers: vec![(
                "Content-Type".to_owned(),
                "text/plain; charset=utf-8".to_owned(),
            )],
            body: "404 page not found\n".to_owned(),
        }
    }
}

/// Serves on a local port until the test process ends. Each connection carries
/// one request, which `handler` answers; every request is also sent on the
/// returned channel. Returns the base URL, such as `http://127.0.0.1:4000`.
pub(crate) fn serve(
    handler: impl Fn(&Request) -> Reply + Send + 'static,
) -> (String, Receiver<Request>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a local port");
    let url = format!("http://{}", listener.local_addr().expect("local address"));
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let Some(request) = read_request(&stream) else {
                continue;
            };
            let reply = handler(&request);
            let _ = tx.send(request);
            write_reply(stream, &reply);
        }
    });
    (url, rx)
}

/// Reads one request: the request line, the header fields and a body of
/// `Content-Length` bytes.
fn read_request(stream: &TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_owned();
    let target = parts.next()?.to_owned();
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_owned(), value.trim().to_owned()));
        }
    }
    let length = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).ok()?;
    Some(Request {
        method,
        target,
        headers,
        body,
    })
}

/// Writes `reply` and closes the connection.
fn write_reply(mut stream: TcpStream, reply: &Reply) {
    let mut head = format!("HTTP/1.1 {} {}\r\n", reply.status, reply.reason);
    for (name, value) in &reply.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str(&format!(
        "Content-Length: {}\r\nDate: Thu, 01 Jan 2026 00:00:00 GMT\r\nConnection: close\r\n\r\n",
        reply.body.len()
    ));
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(reply.body.as_bytes());
    let _ = stream.flush();
}
