// Ports: master/rpc.go @ add21b5

//! The `Master` gRPC service: the metadata a node reads at start and each sync,
//! and the progress nodes push. Also the listener the gRPC services are served
//! on, with the mutual TLS that `master.ssl_mode` turns on (Go's
//! `master/master.go` `Serve`, and `util.NewServerCreds`).

use std::io;
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};

use gorse_common::gotime;
use gorse_common::monitor;
use gorse_protocol::master_server::Master as MasterService;
use gorse_protocol::{Meta, NodeInfo, NodeType, PushProgressRequest, PushProgressResponse};
use gorse_storage::meta::Node;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;
use tonic::transport::server::Connected;
use tonic::{Request, Response, Status};

use crate::master::Master;

/// The name Go's `NodeType.String()` gives a node type. A value the enum does
/// not name is printed as its number.
fn node_type_name(value: i32) -> String {
    NodeType::try_from(value).map_or_else(|_| value.to_string(), |t| t.as_str_name().to_owned())
}

/// Maps a failure to the status a gRPC handler returns for a plain Go error:
/// code `Unknown` with the error text.
fn unknown(err: impl std::fmt::Display) -> Status {
    Status::unknown(err.to_string())
}

#[tonic::async_trait]
impl MasterService for Master {
    /// Go's `GetMeta`: registers the node, then answers with the config, the
    /// model ids, and the names of the workers and servers in the meta store.
    async fn get_meta(&self, request: Request<NodeInfo>) -> Result<Response<Meta>, Status> {
        let node_info = request.into_inner();
        // register node
        let node = Node {
            uuid: node_info.uuid.clone(),
            hostname: node_info.hostname.clone(),
            r#type: node_type_name(node_info.node_type),
            version: node_info.binary_version.clone(),
            update_time: gotime::now().utc(),
        };
        let meta_store = self.meta_store();
        meta_store.update_node(&node).await.map_err(unknown)?;
        // marshall config
        let config = self.config();
        let config_json = config.marshal_json().map_err(unknown)?;
        // save ranking model version
        let collaborative_filtering_model_id = self
            .collaborative_filtering
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .meta
            .id;
        // save click model version
        let click_through_rate_model_id = self
            .click_through_rate
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .meta
            .id;
        // collect nodes
        let mut workers = Vec::new();
        let mut servers = Vec::new();
        for n in meta_store.list_nodes().await.map_err(unknown)? {
            match n.r#type.as_str() {
                "Worker" => workers.push(n.uuid),
                "Server" => servers.push(n.uuid),
                _ => {}
            }
        }
        Ok(Response::new(Meta {
            config: config_json,
            collaborative_filtering_model_id,
            click_through_rate_model_id,
            me: node_info.uuid,
            workers,
            servers,
        }))
    }

    /// Go's `PushProgress`: stores the progress of one tracer. Every entry must
    /// name the same tracer.
    async fn push_progress(
        &self,
        request: Request<PushProgressRequest>,
    ) -> Result<Response<PushProgressResponse>, Status> {
        let in_progress = request.into_inner();
        // check empty progress
        let Some(first) = in_progress.progress.first() else {
            return Ok(Response::new(PushProgressResponse {}));
        };
        // check tracers
        let tracer = first.tracer.clone();
        for p in &in_progress.progress {
            if p.tracer != tracer {
                return Err(Status::unknown(format!(
                    "tracers must be the same, expect {tracer}, got {}",
                    p.tracer
                )));
            }
        }
        // store progress
        self.store_remote_progress(&tracer, monitor::decode_progress(&in_progress));
        Ok(Response::new(PushProgressResponse {}))
    }
}

/// A gRPC connection: plain HTTP/2, or HTTP/2 over the mutual TLS of
/// `util.NewServerCreds`.
pub(crate) enum Conn {
    /// A connection without TLS.
    Plain(TcpStream),
    /// A connection whose TLS handshake has completed.
    Tls(Box<TlsStream<TcpStream>>),
}

impl AsyncRead for Conn {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Conn::Plain(stream) => Pin::new(stream).poll_read(cx, buf),
            Conn::Tls(stream) => Pin::new(stream.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Conn {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Conn::Plain(stream) => Pin::new(stream).poll_write(cx, buf),
            Conn::Tls(stream) => Pin::new(stream.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Conn::Plain(stream) => Pin::new(stream).poll_flush(cx),
            Conn::Tls(stream) => Pin::new(stream.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Conn::Plain(stream) => Pin::new(stream).poll_shutdown(cx),
            Conn::Tls(stream) => Pin::new(stream.as_mut()).poll_shutdown(cx),
        }
    }
}

impl Connected for Conn {
    type ConnectInfo = ();

    fn connect_info(&self) -> Self::ConnectInfo {}
}

/// The connections the gRPC server serves on `listener`. With `acceptor`, each
/// connection completes its TLS handshake before it is handed over, and a
/// failed handshake drops that connection only. The accepting stops when `stop`
/// is sent, as Go's `GracefulStop` closes the listener.
pub(crate) fn incoming(
    listener: TcpListener,
    acceptor: Option<TlsAcceptor>,
    mut stop: watch::Receiver<bool>,
) -> tokio_stream::wrappers::ReceiverStream<Result<Conn, io::Error>> {
    let (tx, rx) = tokio::sync::mpsc::channel(16);
    tokio::spawn(async move {
        loop {
            let accepted = tokio::select! {
                _ = stop.wait_for(|stop| *stop) => return,
                accepted = listener.accept() => accepted,
            };
            let Ok((stream, _)) = accepted else { continue };
            let Some(acceptor) = acceptor.clone() else {
                if tx.send(Ok(Conn::Plain(stream))).await.is_err() {
                    return;
                }
                continue;
            };
            let tx = tx.clone();
            tokio::spawn(async move {
                if let Ok(tls) = acceptor.accept(stream).await {
                    let _ = tx.send(Ok(Conn::Tls(Box::new(tls)))).await;
                }
            });
        }
    });
    tokio_stream::wrappers::ReceiverStream::new(rx)
}

#[cfg(test)]
mod tests;
