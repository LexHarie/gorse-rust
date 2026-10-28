// Ports: the parts of net/http that common/decision, common/reranker and
// common/mock use: a POST that a Go context can cancel (http.NewRequestWithContext
// and Client.Do), and the HTTP/1.1 server the mock servers answer on (http.Server
// with the go-restful handlers). See porting/notes/WP-1-12.md (D-WP12-1).

//! Client and server halves of the HTTP the decision, reranker and OpenAI
//! packages speak. The server is a test double, so it handles one request per
//! connection with a `Content-Length` body and answers with `Connection: close`.

use std::io::{self, BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread;
use std::time::Duration;

use crate::gocontext::{self, ContextError};

/// A reply to [`post`]: the status code and the whole body, whatever the status.
#[derive(Debug)]
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) body: Vec<u8>,
}

/// Why [`post`] gave no reply.
#[derive(Debug)]
pub(crate) enum PostError {
    /// The context was done before the reply arrived (Go's `ctx.Err()`).
    Context(ContextError),
    /// The request failed to send, or its reply could not be read.
    Transport(reqwest::Error),
}

/// Go's `http.DefaultClient`, shared by every request.
fn client() -> &'static reqwest::blocking::Client {
    static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::blocking::Client::new)
}

/// POSTs `body` to `url` with `headers` and returns the reply, whatever its
/// status. A context that is done before the reply comes back ends the call,
/// as Go's cancelled request does. The request itself then finishes on its own
/// thread and its reply is dropped.
pub(crate) fn post(
    ctx: &gocontext::Context,
    url: &str,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
) -> Result<Reply, PostError> {
    if let Some(err) = ctx.err() {
        return Err(PostError::Context(err));
    }
    let (sender, replies) = mpsc::channel();
    let url = url.to_owned();
    thread::spawn(move || {
        let mut request = client().post(url).body(body);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let reply = request.send().and_then(|response| {
            let status = response.status().as_u16();
            let body = response.bytes()?.to_vec();
            Ok(Reply { status, body })
        });
        // The receiver is gone only when the caller's context ended first.
        let _ = sender.send(reply);
    });
    loop {
        match replies.recv_timeout(Duration::from_millis(10)) {
            Ok(reply) => return reply.map_err(PostError::Transport),
            Err(RecvTimeoutError::Timeout) => {
                if let Some(err) = ctx.err() {
                    return Err(PostError::Context(err));
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                panic!("the request thread ended without a reply")
            }
        }
    }
}

/// A request a [`Server`] receives.
#[derive(Debug)]
pub(crate) struct Request {
    pub(crate) method: String,
    pub(crate) path: String,
    // Only the tests read headers: the mock servers answer without them.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Vec<u8>,
}

impl Request {
    /// The value of the first header named `name`, ignoring case.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// A response a [`Server`] sends.
#[derive(Debug)]
pub(crate) struct Response {
    pub(crate) status: u16,
    pub(crate) content_type: &'static str,
    pub(crate) body: Vec<u8>,
}

/// A listener on 127.0.0.1 that answers requests with a handler until closed.
#[derive(Debug)]
pub(crate) struct Server {
    listener: TcpListener,
    addr: SocketAddr,
    closed: AtomicBool,
}

impl Server {
    /// Binds a free port on 127.0.0.1.
    pub(crate) fn bind() -> io::Result<Server> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        Ok(Server {
            listener,
            addr,
            closed: AtomicBool::new(false),
        })
    }

    /// The address the server listens on.
    pub(crate) fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Answers connections with `handler` until [`Server::close`]. Each
    /// connection is served on its own thread; `serve` returns once they all
    /// have finished, and returns `Ok(())` after a close.
    pub(crate) fn serve(&self, handler: &(dyn Fn(Request) -> Response + Sync)) -> io::Result<()> {
        thread::scope(|scope| {
            for stream in self.listener.incoming() {
                if self.closed.load(Ordering::SeqCst) {
                    break;
                }
                let stream = stream?;
                scope.spawn(move || {
                    // A connection that fails mid-request has no one to report to.
                    let _ = answer(stream, handler);
                });
            }
            Ok(())
        })
    }

    /// Stops [`Server::serve`]. The connection this makes wakes the accept loop,
    /// so a close before `serve` also takes effect.
    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
    }
}

/// The server half of Go's `Start`/`Ready`: a server that is bound once and
/// can be waited for by other threads.
#[derive(Debug, Default)]
pub(crate) struct Lifecycle {
    server: Mutex<Option<Arc<Server>>>,
    bound: Condvar,
}

impl Lifecycle {
    /// Binds the listener and publishes it to waiters (Go's `net.Listen` followed
    /// by `close(ready)`).
    pub(crate) fn bind(&self) -> io::Result<Arc<Server>> {
        let server = Arc::new(Server::bind()?);
        *lock(&self.server) = Some(Arc::clone(&server));
        self.bound.notify_all();
        Ok(server)
    }

    /// Waits until [`Lifecycle::bind`] has run (Go's `Ready`).
    pub(crate) fn wait_ready(&self) -> Arc<Server> {
        let mut server = lock(&self.server);
        loop {
            if let Some(bound) = server.as_ref() {
                return Arc::clone(bound);
            }
            server = self
                .bound
                .wait(server)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// The bound server, if [`Lifecycle::bind`] has run.
    pub(crate) fn get(&self) -> Option<Arc<Server>> {
        lock(&self.server).clone()
    }
}

/// Locks `mutex`; no code that can panic runs under these locks.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Reads one request from `stream`, runs the handler, and writes its reply.
fn answer(mut stream: TcpStream, handler: &(dyn Fn(Request) -> Response + Sync)) -> io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let response = match read_request(&mut reader) {
        Ok(Some(request)) => handler(request),
        Ok(None) => return Ok(()),
        Err(err) => Response {
            status: 400,
            content_type: "text/plain; charset=utf-8",
            body: err.to_string().into_bytes(),
        },
    };
    write_response(&mut stream, &response)
}

/// Reads a request line, its headers and a `Content-Length` body. `Ok(None)`
/// is a connection that closed before sending anything.
fn read_request(reader: &mut impl BufRead) -> io::Result<Option<Request>> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let (Some(method), Some(path)) = (parts.next(), parts.next()) else {
        return Err(bad_request("malformed request line"));
    };
    let (method, path) = (method.to_owned(), path.to_owned());
    let mut headers = Vec::new();
    loop {
        line.clear();
        reader.read_line(&mut line)?;
        let text = line.trim_end_matches(['\r', '\n']);
        if text.is_empty() {
            break;
        }
        let (name, value) = text
            .split_once(':')
            .ok_or_else(|| bad_request("malformed header"))?;
        headers.push((name.trim().to_owned(), value.trim().to_owned()));
    }
    if headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("transfer-encoding"))
    {
        return Err(bad_request("chunked request bodies are not supported"));
    }
    let length = match headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
    {
        Some((_, value)) => value
            .parse::<usize>()
            .map_err(|_| bad_request("malformed Content-Length"))?,
        None => 0,
    };
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    Ok(Some(Request {
        method,
        path,
        headers,
        body,
    }))
}

fn bad_request(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Writes `response` with its length and `Connection: close`.
fn write_response(stream: &mut TcpStream, response: &Response) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.status,
        reason(response.status),
        response.content_type,
        response.body.len(),
    )?;
    stream.write_all(&response.body)?;
    stream.flush()
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        422 => "Unprocessable Entity",
        500 => "Internal Server Error",
        _ => "Unknown",
    }
}
