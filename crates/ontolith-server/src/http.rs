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

    fn write_to(&self, stream: &mut TcpStream) -> std::io::Result<()> {
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
    running: Arc<AtomicBool>,
    accepted: Arc<AtomicU64>,
}

impl HttpServer {
    pub fn new(handler: Handler) -> Self {
        Self {
            handler,
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
        listener.set_nonblocking(false)?;
        self.running.store(true, Ordering::SeqCst);
        while self.running.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _)) => {
                    self.accepted.fetch_add(1, Ordering::Relaxed);
                    let handler = Arc::clone(&self.handler);
                    thread::spawn(move || {
                        let _ = handle_connection(stream, handler);
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

fn handle_connection(mut stream: TcpStream, handler: Handler) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let req = match read_request(&mut stream) {
        Ok(r) => r,
        Err(err) => {
            let resp = HttpResponse::text(400, "Bad Request", format!("bad request: {err}"));
            let _ = resp.write_to(&mut stream);
            return Ok(());
        }
    };
    let resp = handler(req);
    resp.write_to(&mut stream)
}

fn read_request(stream: &mut TcpStream) -> Result<HttpRequest, String> {
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
}
