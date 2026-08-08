//! Minimal HTTP/1.1 raft RPC transport (L4, ADR-0004 decision 2).
//!
//! In-tree peer transport without third-party runtime dependencies, mirroring
//! the minimal HTTP stack used by L5 (`ontolith-server::http`): peers POST
//! serde_json-encoded openraft RPCs to `/internal/raft/{vote,append-entries,
//! install-snapshot}` authenticated by a shared cluster secret
//! (`Authorization: Bearer <secret>`).

use super::{NodeId, TypeConfig};
use crate::domain::{LogEntry, LogPayload, ShardId, SlotRange};
use ontolith_storage::domain::SnapshotRef;
use openraft::BasicNode;
use openraft::error::{NetworkError, RPCError, RaftError, RemoteError};
use openraft::network::{RPCOption, RaftNetwork, RaftNetworkFactory};
use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, InstallSnapshotRequest, InstallSnapshotResponse,
    VoteRequest, VoteResponse,
};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

type RaftHandle = Arc<openraft::Raft<TypeConfig>>;
type NetError = RPCError<NodeId, BasicNode, RaftError<NodeId>>;
type SnapshotNetError =
    RPCError<NodeId, BasicNode, RaftError<NodeId, openraft::error::InstallSnapshotError>>;

/// Data-plane snapshot IO hook (P4-03): the owning process plugs its L2
/// storage so `transfer_snapshot` can move real snapshot bytes between nodes.
pub trait DataPlaneSnapshotIo: Send + Sync {
    /// Serialize the snapshot referenced by `snapshot` into transfer bytes.
    fn export_snapshot(&self, snapshot: &SnapshotRef) -> Result<Vec<u8>, String>;
    /// Apply transferred snapshot bytes for a slot range into local storage.
    fn import_snapshot(
        &self,
        shard_id: ShardId,
        slots: SlotRange,
        bytes: &[u8],
    ) -> Result<(), String>;
}

/// Wire request for `/internal/raft/transfer-snapshot` (P4-03).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TransferSnapshotRequest {
    pub shard_id: u32,
    pub slots_start: u32,
    pub slots_end: u32,
    pub snapshot_id: u64,
    pub bytes: Vec<u8>,
}

// ---------------------------------------------------------------------------
// HTTP message primitives (mirrors `ontolith-server::http`).
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct HttpRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl HttpRequest {
    fn header(&self, name: &str) -> Option<&str> {
        let want = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| k.to_ascii_lowercase() == want)
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Debug)]
struct HttpResponse {
    status: u16,
    reason: &'static str,
    body: Vec<u8>,
}

impl HttpResponse {
    fn new(status: u16, reason: &'static str, body: Vec<u8>) -> Self {
        Self {
            status,
            reason,
            body,
        }
    }

    fn text(status: u16, reason: &'static str, body: impl Into<String>) -> Self {
        Self::new(status, reason, body.into().into_bytes())
    }

    fn write_to<W: Write>(&self, stream: &mut W) -> std::io::Result<()> {
        let mut out = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.status,
            self.reason,
            self.body.len()
        );
        out.push_str(&String::from_utf8_lossy(&self.body));
        stream.write_all(out.as_bytes())?;
        stream.flush()
    }
}

fn read_request<R: Read>(reader: &mut R) -> Result<HttpRequest, String> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    let header_end = loop {
        if let Some(pos) = find_header_end(&buf) {
            break pos;
        }
        let n = reader.read(&mut tmp).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("unexpected EOF before request headers".into());
        }
        buf.extend_from_slice(&tmp[..n]);
    };

    let head = String::from_utf8_lossy(&buf[..header_end]);
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();

    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }

    let content_length = headers
        .get("content-length")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < content_length {
        let n = reader.read(&mut tmp).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(content_length);

    Ok(HttpRequest {
        method,
        path: target,
        headers,
        body,
    })
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn read_response<R: Read>(reader: &mut R) -> Result<HttpResponse, String> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    let header_end = loop {
        if let Some(pos) = find_header_end(&buf) {
            break pos;
        }
        let n = reader.read(&mut tmp).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("unexpected EOF before response headers".into());
        }
        buf.extend_from_slice(&tmp[..n]);
    };

    let head = String::from_utf8_lossy(&buf[..header_end]);
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|v| v.parse::<u16>().ok())
        .ok_or_else(|| format!("malformed status line: {status_line}"))?;

    let mut headers = HashMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }

    let content_length = headers
        .get("content-length")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < content_length {
        let n = reader.read(&mut tmp).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(content_length);

    Ok(HttpResponse {
        status,
        reason: "RPC",
        body,
    })
}

// ---------------------------------------------------------------------------
// Server side.
// ---------------------------------------------------------------------------

/// HTTP raft RPC server for one node, bound to a concrete local address.
pub struct HttpRaftServer {
    /// The actually-bound listen address (`127.0.0.1:0` resolves to a port).
    pub addr: SocketAddr,
    running: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl HttpRaftServer {
    pub fn spawn(
        listen: SocketAddr,
        secret: String,
        runtime: Arc<tokio::runtime::Runtime>,
        raft: RaftHandle,
        data_plane: Arc<Mutex<Option<Arc<dyn DataPlaneSnapshotIo>>>>,
    ) -> std::io::Result<Self> {
        let listener = TcpListener::bind(listen)?;
        let addr = listener.local_addr()?;
        let running = Arc::new(AtomicBool::new(true));
        let join = {
            let running = Arc::clone(&running);
            let data_plane = Arc::clone(&data_plane);
            thread::spawn(move || serve(listener, secret, runtime, raft, running, data_plane))
        };
        Ok(Self {
            addr,
            running,
            join: Some(join),
        })
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        // Unblock the accept loop before joining (a bare connect is enough to
        // wake `TcpListener::accept`; the handler thread sees EOF and exits).
        let _ = TcpStream::connect(self.addr);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for HttpRaftServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn serve(
    listener: TcpListener,
    secret: String,
    runtime: Arc<tokio::runtime::Runtime>,
    raft: RaftHandle,
    running: Arc<AtomicBool>,
    data_plane: Arc<Mutex<Option<Arc<dyn DataPlaneSnapshotIo>>>>,
) {
    while running.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                let secret = secret.clone();
                let runtime = Arc::clone(&runtime);
                let raft = Arc::clone(&raft);
                let data_plane = Arc::clone(&data_plane);
                thread::spawn(move || {
                    let _ = handle_connection(stream, &secret, &runtime, &raft, &data_plane);
                });
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => {
                if running.load(Ordering::SeqCst) {
                    eprintln!("ontolith raft http accept error: {err}");
                    thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }
}

fn handle_connection(
    mut stream: TcpStream,
    secret: &str,
    runtime: &tokio::runtime::Runtime,
    raft: &openraft::Raft<TypeConfig>,
    data_plane: &Arc<Mutex<Option<Arc<dyn DataPlaneSnapshotIo>>>>,
) -> std::io::Result<()> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    let request = match read_request(&mut stream) {
        Ok(req) => req,
        Err(e) => {
            let resp = HttpResponse::text(400, "Bad Request", format!("malformed request: {e}"));
            let _ = resp.write_to(&mut stream);
            return Ok(());
        }
    };
    let response = route(&request, secret, runtime, raft, data_plane);
    response.write_to(&mut stream)
}

fn route(
    request: &HttpRequest,
    secret: &str,
    runtime: &tokio::runtime::Runtime,
    raft: &openraft::Raft<TypeConfig>,
    data_plane: &Arc<Mutex<Option<Arc<dyn DataPlaneSnapshotIo>>>>,
) -> HttpResponse {
    match request.header("authorization") {
        Some(value) if value == format!("Bearer {secret}") => {}
        _ => {
            return HttpResponse::text(
                401,
                "Unauthorized",
                "missing or invalid raft cluster secret",
            );
        }
    }
    if request.method != "POST" {
        return HttpResponse::text(
            405,
            "Method Not Allowed",
            "raft RPC endpoints accept POST only",
        );
    }
    match request.path.as_str() {
        "/internal/raft/vote" => handle_vote(runtime, raft, &request.body),
        "/internal/raft/append-entries" => handle_append_entries(runtime, raft, &request.body),
        "/internal/raft/install-snapshot" => handle_install_snapshot(runtime, raft, &request.body),
        "/internal/raft/apply" => handle_apply(runtime, raft, &request.body),
        "/internal/raft/transfer-snapshot" => handle_transfer_snapshot(data_plane, &request.body),
        _ => HttpResponse::text(404, "Not Found", "unknown raft RPC endpoint"),
    }
}

fn json_response<T: serde::Serialize>(status: u16, value: &T) -> HttpResponse {
    match serde_json::to_vec(value) {
        Ok(body) => HttpResponse::new(
            status,
            if status == 200 { "OK" } else { "Bad Request" },
            body,
        ),
        Err(e) => HttpResponse::text(
            500,
            "Internal Server Error",
            format!("serialize raft rpc response: {e}"),
        ),
    }
}

fn handle_vote(
    runtime: &tokio::runtime::Runtime,
    raft: &openraft::Raft<TypeConfig>,
    body: &[u8],
) -> HttpResponse {
    let rpc: VoteRequest<NodeId> = match serde_json::from_slice(body) {
        Ok(rpc) => rpc,
        Err(e) => {
            return HttpResponse::text(400, "Bad Request", format!("invalid vote payload: {e}"));
        }
    };
    match runtime.block_on(raft.vote(rpc)) {
        Ok(resp) => json_response(200, &resp),
        Err(e) => json_response(400, &e),
    }
}

fn handle_append_entries(
    runtime: &tokio::runtime::Runtime,
    raft: &openraft::Raft<TypeConfig>,
    body: &[u8],
) -> HttpResponse {
    let rpc: AppendEntriesRequest<TypeConfig> = match serde_json::from_slice(body) {
        Ok(rpc) => rpc,
        Err(e) => {
            return HttpResponse::text(
                400,
                "Bad Request",
                format!("invalid append-entries payload: {e}"),
            );
        }
    };
    match runtime.block_on(raft.append_entries(rpc)) {
        Ok(resp) => json_response(200, &resp),
        Err(e) => json_response(400, &e),
    }
}

fn handle_install_snapshot(
    runtime: &tokio::runtime::Runtime,
    raft: &openraft::Raft<TypeConfig>,
    body: &[u8],
) -> HttpResponse {
    let rpc: InstallSnapshotRequest<TypeConfig> = match serde_json::from_slice(body) {
        Ok(rpc) => rpc,
        Err(e) => {
            return HttpResponse::text(
                400,
                "Bad Request",
                format!("invalid install-snapshot payload: {e}"),
            );
        }
    };
    match runtime.block_on(raft.install_snapshot(rpc)) {
        Ok(resp) => json_response(200, &resp),
        Err(e) => json_response(400, &e),
    }
}

/// Metadata apply RPC (P4-01): the leader commits a [`LogPayload`] as a raft
/// entry and returns the committed [`LogEntry`]. A follower answers `409` with
/// the current leader hint so the caller can forward and retry.
fn handle_apply(
    runtime: &tokio::runtime::Runtime,
    raft: &openraft::Raft<TypeConfig>,
    body: &[u8],
) -> HttpResponse {
    let payload: LogPayload = match serde_json::from_slice(body) {
        Ok(p) => p,
        Err(e) => {
            return HttpResponse::text(400, "Bad Request", format!("invalid apply payload: {e}"));
        }
    };
    let receiver = raft.metrics();
    let metrics = receiver.borrow();
    if metrics.current_leader != Some(metrics.id) {
        let leader = metrics
            .current_leader
            .map(|id| format!("\"leader\":{id}"))
            .unwrap_or_else(|| "\"leader\":null".to_owned());
        return HttpResponse::text(
            409,
            "Conflict",
            format!(r#"{{"error":"not_leader",{leader}}}"#),
        );
    }
    drop(metrics);
    match runtime
        .block_on(raft.client_write::<tokio::sync::oneshot::error::RecvError>(payload.clone()))
    {
        Ok(resp) => json_response(
            200,
            &LogEntry {
                index: resp.log_id.index,
                term: crate::domain::ClusterEpoch::new(resp.log_id.leader_id.term),
                payload,
            },
        ),
        Err(e) => json_response(400, &e),
    }
}

/// Data-plane transfer RPC (P4-03): the target imports a snapshot byte blob
/// for a slot range through its [`DataPlaneSnapshotIo`] hook.
fn handle_transfer_snapshot(
    data_plane: &Arc<Mutex<Option<Arc<dyn DataPlaneSnapshotIo>>>>,
    body: &[u8],
) -> HttpResponse {
    let request: TransferSnapshotRequest = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(e) => {
            return HttpResponse::text(
                400,
                "Bad Request",
                format!("invalid transfer-snapshot payload: {e}"),
            );
        }
    };
    let io = data_plane.lock().unwrap().clone();
    let Some(io) = io else {
        return HttpResponse::text(
            503,
            "Service Unavailable",
            "target node has no data-plane snapshot IO hook installed",
        );
    };
    let slots = SlotRange {
        start: request.slots_start,
        end: request.slots_end,
    };
    match io.import_snapshot(ShardId::new(request.shard_id), slots, &request.bytes) {
        Ok(()) => HttpResponse::text(200, "OK", r#"{"imported":true}"#),
        Err(e) => HttpResponse::text(
            500,
            "Internal Server Error",
            format!("import snapshot failed: {e}"),
        ),
    }
}

// ---------------------------------------------------------------------------
// Client side.
// ---------------------------------------------------------------------------

/// HTTP [`openraft::RaftNetwork`] client for one target node.
#[derive(Clone)]
pub struct HttpRaftClient {
    target: NodeId,
    addr: String,
    secret: String,
    self_node_id: NodeId,
    partition: Arc<Mutex<HashSet<NodeId>>>,
}

impl HttpRaftClient {
    async fn post<Req, Resp, E>(
        &self,
        path: &str,
        rpc: &Req,
    ) -> Result<Resp, RPCError<NodeId, BasicNode, E>>
    where
        Req: serde::Serialize,
        Resp: serde::de::DeserializeOwned,
        E: serde::de::DeserializeOwned + std::error::Error,
    {
        let partitioned = {
            let partition = self.partition.lock().unwrap();
            partition.contains(&self.target) || partition.contains(&self.self_node_id)
        };
        if partitioned {
            return Err(RPCError::Network(NetworkError::new(
                &std::io::Error::other("peer isolated by network partition"),
            )));
        }
        let body = serde_json::to_vec(rpc).map_err(|e| RPCError::Network(NetworkError::new(&e)))?;
        let target = self.target;
        let secret = self.secret.clone();
        let addr = self.addr.clone();
        let path = path.to_owned();

        let raw = tokio::task::spawn_blocking(move || -> Result<(u16, Vec<u8>), String> {
            let host_port = parse_host_port(&addr)?;
            let mut stream = TcpStream::connect_timeout(&host_port, Duration::from_secs(5))
                .map_err(|e| e.to_string())?;
            let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
            let head = format!(
                "POST {path} HTTP/1.1\r\nHost: {host_port}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                secret,
                body.len()
            );
            stream
                .write_all(head.as_bytes())
                .and_then(|_| stream.write_all(&body))
                .and_then(|_| stream.flush())
                .map_err(|e| e.to_string())?;
            let resp = read_response(&mut stream).map_err(|e| e.to_string())?;
            Ok((resp.status, resp.body))
        })
        .await
        .map_err(|e| RPCError::Network(NetworkError::new(&std::io::Error::other(format!("raft rpc task: {e}")))))?;
        let (status, resp_body) =
            raw.map_err(|msg| RPCError::Network(NetworkError::new(&std::io::Error::other(msg))))?;

        if status == 200 {
            serde_json::from_slice(&resp_body).map_err(|e| RPCError::Network(NetworkError::new(&e)))
        } else {
            let raft_error: E = serde_json::from_slice(&resp_body)
                .map_err(|e| RPCError::Network(NetworkError::new(&e)))?;
            Err(RPCError::RemoteError(RemoteError::new(target, raft_error)))
        }
    }
}

fn parse_host_port(addr: &str) -> Result<SocketAddr, String> {
    let addr = addr
        .strip_prefix("http://")
        .unwrap_or(addr)
        .trim_end_matches('/');
    if addr.contains("://") {
        return Err(format!("unsupported raft peer scheme in {addr:?}"));
    }
    addr.to_socket_addrs()
        .ok()
        .and_then(|mut it| it.next())
        .ok_or_else(|| format!("cannot resolve raft peer address {addr:?}"))
}

/// Blocking HTTP POST of a metadata [`LogPayload`] to a peer's apply endpoint
/// (P4-01 forwarding). Returns `(status, body)`.
pub(crate) fn apply_on_peer(
    addr: &str,
    secret: &str,
    payload: &LogPayload,
) -> Result<(u16, Vec<u8>), String> {
    let host_port = parse_host_port(addr)?;
    let body = serde_json::to_vec(payload).map_err(|e| format!("serialize apply payload: {e}"))?;
    let mut stream = TcpStream::connect_timeout(&host_port, Duration::from_secs(5))
        .map_err(|e| format!("connect raft peer {addr}: {e}"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    let head = format!(
        "POST /internal/raft/apply HTTP/1.1\r\nHost: {host_port}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        secret,
        body.len()
    );
    stream
        .write_all(head.as_bytes())
        .and_then(|_| stream.write_all(&body))
        .and_then(|_| stream.flush())
        .map_err(|e| format!("write apply request: {e}"))?;
    let resp = read_response(&mut stream).map_err(|e| format!("read apply response: {e}"))?;
    Ok((resp.status, resp.body))
}

/// Blocking HTTP POST of a snapshot transfer to a peer (P4-03).
pub(crate) fn transfer_snapshot_on_peer(
    addr: &str,
    secret: &str,
    request: &TransferSnapshotRequest,
) -> Result<(u16, Vec<u8>), String> {
    let host_port = parse_host_port(addr)?;
    let body =
        serde_json::to_vec(request).map_err(|e| format!("serialize transfer request: {e}"))?;
    let mut stream = TcpStream::connect_timeout(&host_port, Duration::from_secs(5))
        .map_err(|e| format!("connect raft peer {addr}: {e}"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    let head = format!(
        "POST /internal/raft/transfer-snapshot HTTP/1.1\r\nHost: {host_port}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        secret,
        body.len()
    );
    stream
        .write_all(head.as_bytes())
        .and_then(|_| stream.write_all(&body))
        .and_then(|_| stream.flush())
        .map_err(|e| format!("write transfer request: {e}"))?;
    let resp = read_response(&mut stream).map_err(|e| format!("read transfer response: {e}"))?;
    Ok((resp.status, resp.body))
}

impl RaftNetwork<TypeConfig> for HttpRaftClient {
    async fn append_entries(
        &mut self,
        rpc: AppendEntriesRequest<TypeConfig>,
        _option: RPCOption,
    ) -> Result<AppendEntriesResponse<NodeId>, NetError> {
        self.post("/internal/raft/append-entries", &rpc).await
    }

    async fn install_snapshot(
        &mut self,
        rpc: InstallSnapshotRequest<TypeConfig>,
        _option: RPCOption,
    ) -> Result<InstallSnapshotResponse<NodeId>, SnapshotNetError> {
        self.post("/internal/raft/install-snapshot", &rpc).await
    }

    async fn vote(
        &mut self,
        rpc: VoteRequest<NodeId>,
        _option: RPCOption,
    ) -> Result<VoteResponse<NodeId>, NetError> {
        self.post("/internal/raft/vote", &rpc).await
    }
}

/// [`openraft::RaftNetworkFactory`] that routes RPCs over HTTP to
/// `BasicNode::addr` with the shared cluster secret.
#[derive(Clone)]
pub struct HttpRaftFactory {
    secret: String,
    self_node_id: NodeId,
    partition: Arc<Mutex<HashSet<NodeId>>>,
}

impl HttpRaftFactory {
    pub fn new(
        secret: impl Into<String>,
        self_node_id: NodeId,
        partition: Arc<Mutex<HashSet<NodeId>>>,
    ) -> Self {
        Self {
            secret: secret.into(),
            self_node_id,
            partition,
        }
    }
}

impl RaftNetworkFactory<TypeConfig> for HttpRaftFactory {
    type Network = HttpRaftClient;

    async fn new_client(&mut self, target: NodeId, node: &BasicNode) -> Self::Network {
        HttpRaftClient {
            target,
            addr: node.addr.clone(),
            secret: self.secret.clone(),
            self_node_id: self.self_node_id,
            partition: Arc::clone(&self.partition),
        }
    }
}
