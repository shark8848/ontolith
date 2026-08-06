//! Minimal HTTP/1.1 server (L5) without third-party runtime deps.
//!
//! Supports request-line + headers + optional Content-Length body, enough for
//! /health, /metrics, /sparql, /explain, /audit.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// In-process TLS termination configuration (rustls server config).
#[derive(Clone)]
pub struct TlsServerConfig {
    inner: Arc<rustls::ServerConfig>,
}

impl TlsServerConfig {
    /// Build a TLS server config from PEM-encoded certificate chain and private key.
    pub fn from_pem(cert_pem: &[u8], key_pem: &[u8]) -> Result<Self, String> {
        let certs = load_pem_certs(cert_pem)?;
        let key = load_pem_key(key_pem)?;
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(|e| format!("invalid TLS certificate/key pair: {e}"))?;
        Ok(Self {
            inner: Arc::new(config),
        })
    }

    fn rustls_config(&self) -> Arc<rustls::ServerConfig> {
        Arc::clone(&self.inner)
    }
}

fn load_pem_certs(cert_pem: &[u8]) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>, String> {
    let mut reader = std::io::BufReader::new(cert_pem);
    let certs: Result<Vec<_>, _> = rustls_pemfile::certs(&mut reader)
        .collect();
    let certs = certs.map_err(|e| format!("read TLS certificate: {e}"))?;
    if certs.is_empty() {
        return Err("TLS certificate PEM contains no certificates".to_owned());
    }
    Ok(certs)
}

fn load_pem_key(key_pem: &[u8]) -> Result<rustls::pki_types::PrivateKeyDer<'static>, String> {
    let mut reader = std::io::BufReader::new(key_pem);
    let key = rustls_pemfile::private_key(&mut reader)
        .map_err(|e| format!("read TLS private key: {e}"))?
        .ok_or_else(|| "TLS private key PEM contains no key".to_owned())?;
    Ok(key)
}

#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl HttpRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        let want = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| k.to_ascii_lowercase() == want)
            .map(|(_, v)| v.as_str())
    }

    pub fn body_str(&self) -> &str {
        std::str::from_utf8(&self.body).unwrap_or("")
    }
}

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub reason: &'static str,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn new(status: u16, reason: &'static str, body: impl Into<Vec<u8>>) -> Self {
        let body = body.into();
        Self {
            status,
            reason,
            headers: vec![
                ("Content-Length".into(), body.len().to_string()),
                ("Connection".into(), "close".into()),
            ],
            body,
        }
    }

    pub fn text(status: u16, reason: &'static str, body: impl Into<String>) -> Self {
        let mut resp = Self::new(status, reason, body.into().into_bytes());
        resp.headers
            .push(("Content-Type".into(), "text/plain; charset=utf-8".into()));
        resp
    }

    pub fn json(status: u16, reason: &'static str, body: impl Into<String>) -> Self {
        let mut resp = Self::new(status, reason, body.into().into_bytes());
        resp.headers.push((
            "Content-Type".into(),
            "application/json; charset=utf-8".into(),
        ));
        resp
    }

    pub fn html_like_prometheus(body: impl Into<String>) -> Self {
        let mut resp = Self::new(200, "OK", body.into().into_bytes());
        resp.headers.push((
            "Content-Type".into(),
            "text/plain; version=0.0.4; charset=utf-8".into(),
        ));
        resp
    }

    fn write_to<W: Write>(&self, stream: &mut W) -> std::io::Result<()> {
        let mut out = format!("HTTP/1.1 {} {}\r\n", self.status, self.reason);
        for (k, v) in &self.headers {
            out.push_str(k);
            out.push_str(": ");
            out.push_str(v);
            out.push_str("\r\n");
        }
        out.push_str("\r\n");
        stream.write_all(out.as_bytes())?;
        stream.write_all(&self.body)?;
        stream.flush()
    }
}

pub type Handler = Arc<dyn Fn(HttpRequest) -> HttpResponse + Send + Sync + 'static>;

#[derive(Clone)]
pub struct HttpServer {
    handler: Handler,
    tls: Option<TlsServerConfig>,
    running: Arc<AtomicBool>,
    accepted: Arc<AtomicU64>,
}

impl HttpServer {
    pub fn new(handler: Handler) -> Self {
        Self {
            handler,
            tls: None,
            running: Arc::new(AtomicBool::new(false)),
            accepted: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn with_tls(handler: Handler, tls: TlsServerConfig) -> Self {
        Self {
            handler,
            tls: Some(tls),
            running: Arc::new(AtomicBool::new(false)),
            accepted: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn accepted(&self) -> u64 {
        self.accepted.load(Ordering::Relaxed)
    }

    pub fn stop_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.running)
    }

    /// Block and serve until `running` is set false or accept fails fatally.
    pub fn serve<A: ToSocketAddrs>(&self, addr: A) -> std::io::Result<()> {
        let listener = TcpListener::bind(addr)?;
        self.serve_listener(listener)
    }

    /// Serve an already-bound listener (used by tests and callers that need
    /// the concrete bound address before serving).
    pub(crate) fn serve_listener(&self, listener: TcpListener) -> std::io::Result<()> {
        listener.set_nonblocking(false)?;
        self.running.store(true, Ordering::SeqCst);
        while self.running.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _)) => {
                    self.accepted.fetch_add(1, Ordering::Relaxed);
                    let handler = Arc::clone(&self.handler);
                    let tls = self.tls.clone();
                    thread::spawn(move || {
                        let _ = handle_connection(stream, handler, tls);
                    });
                }
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(err) => {
                    if !self.running.load(Ordering::SeqCst) {
                        break;
                    }
                    // brief backoff on transient errors
                    eprintln!("ontolith-server accept error: {err}");
                    thread::sleep(Duration::from_millis(50));
                }
            }
        }
        Ok(())
    }
}

fn handle_connection(
    stream: TcpStream,
    handler: Handler,
    tls: Option<TlsServerConfig>,
) -> std::io::Result<()> {
    if let Some(tls) = tls {
        let mut conn = rustls::ServerConnection::new(tls.rustls_config())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let mut stream = stream;
        stream.set_read_timeout(Some(Duration::from_secs(30)))?;
        stream.set_write_timeout(Some(Duration::from_secs(30)))?;
        let result = {
            let mut tls_stream = rustls::Stream::new(&mut conn, &mut stream);
            handle_connection_io(&mut tls_stream, handler)
        };
        // Send and flush TLS close_notify before dropping the connection so
        // well-behaved clients observe a clean EOF.
        conn.send_close_notify();
        let _ = conn.complete_io(&mut stream);
        return result;
    }

    let mut stream = stream;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    handle_connection_io(&mut stream, handler)
}

fn handle_connection_io<S: Read + Write>(
    stream: &mut S,
    handler: Handler,
) -> std::io::Result<()> {
    let req = match read_request(stream) {
        Ok(r) => r,
        Err(err) => {
            let resp = HttpResponse::text(400, "Bad Request", format!("bad request: {err}"));
            let _ = resp.write_to(stream);
            return Ok(());
        }
    };
    let resp = handler(req);
    resp.write_to(stream)
}

fn read_request<S: Read>(stream: &mut S) -> Result<HttpRequest, String> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    // read until header end
    loop {
        let n = stream.read(&mut tmp).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 1024 * 1024 {
            return Err("headers too large".into());
        }
    }
    let header_end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| "incomplete headers".to_string())?;
    let header_bytes = &buf[..header_end];
    let header_text = std::str::from_utf8(header_bytes).map_err(|_| "headers not utf-8")?;
    let mut lines = header_text.split("\r\n");
    let request_line = lines.next().ok_or("missing request line")?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or("missing method")?.to_owned();
    let target = parts.next().ok_or("missing path")?.to_owned();
    let (path, query) = split_target(&target);

    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_owned(), v.trim().to_owned());
        }
    }

    let content_length = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);

    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut tmp).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(content_length);

    Ok(HttpRequest {
        method,
        path,
        query,
        headers,
        body,
    })
}

fn split_target(target: &str) -> (String, HashMap<String, String>) {
    if let Some((path, q)) = target.split_once('?') {
        let mut map = HashMap::new();
        for pair in q.split('&') {
            if pair.is_empty() {
                continue;
            }
            if let Some((k, v)) = pair.split_once('=') {
                map.insert(url_decode(k), url_decode(v));
            } else {
                map.insert(url_decode(pair), String::new());
            }
        }
        (path.to_owned(), map)
    } else {
        (target.to_owned(), HashMap::new())
    }
}

fn url_decode(input: &str) -> String {
    let mut out = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = &input[i + 1..i + 3];
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    out.push(v);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_target_decodes_query_params() {
        let (p, q) = split_target("/sparql?query=SELECT%20%2A&explain=1");
        assert_eq!(p, "/sparql");
        assert_eq!(q.get("query").map(String::as_str), Some("SELECT *"));
        assert_eq!(q.get("explain").map(String::as_str), Some("1"));
    }

    #[test]
    fn tls_config_from_pem_rejects_garbage() {
        assert!(TlsServerConfig::from_pem(b"not a certificate", b"not a key").is_err());
    }

    #[test]
    fn tls_server_serves_https_request() {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()])
            .expect("generate self-signed cert");
        let tls = TlsServerConfig::from_pem(
            cert.cert.pem().as_bytes(),
            cert.key_pair.serialize_pem().as_bytes(),
        )
        .expect("load tls config from pem");

        let handler: Handler = Arc::new(|req: HttpRequest| {
            HttpResponse::json(200, "OK", format!(r#"{{"path":"{}"}}"#, req.path))
        });
        let server = HttpServer::with_tls(handler, tls);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        let server_thread = {
            let server = server.clone();
            thread::spawn(move || server.serve_listener(listener))
        };

        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(cert.cert.der().clone())
            .expect("add root cert");
        let client_config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let server_name =
            rustls::pki_types::ServerName::try_from("localhost").expect("server name");
        let mut client =
            rustls::ClientConnection::new(Arc::new(client_config), server_name)
                .expect("client connection");
        let mut stream = TcpStream::connect(addr).expect("connect to tls server");
        let mut tls_stream = rustls::Stream::new(&mut client, &mut stream);
        tls_stream
            .write_all(
                b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            )
            .expect("write request");
        let mut buf = Vec::new();
        tls_stream.read_to_end(&mut buf).expect("read response");
        let text = String::from_utf8(buf).expect("utf8 response");
        assert!(text.starts_with("HTTP/1.1 200 OK"), "got: {text}");
        assert!(text.contains("\"path\":\"/health\""), "got: {text}");

        server.stop_flag().store(false, Ordering::SeqCst);
        let _ = TcpStream::connect(addr);
        let _ = server_thread.join().expect("server thread");
    }
}
