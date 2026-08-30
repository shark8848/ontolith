//! JSON-LD remote `@context` loader (WBS-02 extension).
//!
//! The parser itself performs no network I/O; this module supplies the L5
//! ingest path with a minimal synchronous HTTP GET loader (same in-tree
//! transport style as the OIDC JWKS fetch), enabled via
//! `ONTOLITH_JSONLD_REMOTE_CONTEXT=1`. Only `http://` URLs are supported;
//! `https://` and other schemes fail with a clear error.

use ontolith_core::error::OntolithError;
use ontolith_parser::infrastructure::RemoteContextLoader;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// Whether the ingest path should resolve remote `@context` URLs
/// (`ONTOLITH_JSONLD_REMOTE_CONTEXT` = `1`/`true`).
pub fn remote_context_enabled() -> bool {
    std::env::var("ONTOLITH_JSONLD_REMOTE_CONTEXT")
        .ok()
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

/// Minimal HTTP-only remote context fetcher.
#[derive(Debug, Clone, Default)]
pub struct HttpRemoteContextLoader;

impl RemoteContextLoader for HttpRemoteContextLoader {
    fn load(&self, url: &str) -> Result<String, OntolithError> {
        let rest = url
            .strip_prefix("http://")
            .ok_or(OntolithError::Unsupported(
                "json-ld remote @context: only http:// URLs supported",
            ))?;
        let (host_port, path) = match rest.find('/') {
            Some(idx) => (&rest[..idx], &rest[idx..]),
            None => (rest, "/"),
        };
        let addr = host_port
            .to_socket_addrs()
            .ok()
            .and_then(|mut it| it.next())
            .ok_or_else(|| {
                OntolithError::Failed(format!("cannot resolve json-ld context host {host_port:?}"))
            })?;
        let mut stream =
            TcpStream::connect_timeout(&addr, Duration::from_secs(5)).map_err(|e| {
                OntolithError::Failed(format!("connect json-ld context host {host_port}: {e}"))
            })?;
        let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
        let head = format!(
            "GET {path} HTTP/1.1\r\nHost: {host_port}\r\nAccept: application/ld+json, application/json\r\nConnection: close\r\n\r\n"
        );
        stream
            .write_all(head.as_bytes())
            .and_then(|_| stream.flush())
            .map_err(|e| OntolithError::Failed(format!("write json-ld context request: {e}")))?;

        let mut raw = Vec::new();
        stream
            .read_to_end(&mut raw)
            .map_err(|e| OntolithError::Failed(format!("read json-ld context response: {e}")))?;
        let text = String::from_utf8_lossy(&raw);
        let (head, body) = text.split_once("\r\n\r\n").unwrap_or(("", &text));
        let status = head
            .lines()
            .next()
            .unwrap_or_default()
            .split_whitespace()
            .nth(1)
            .unwrap_or_default();
        if !status.starts_with('2') {
            return Err(OntolithError::Failed(format!(
                "json-ld remote @context {url} returned HTTP {status}"
            )));
        }
        Ok(body.to_owned())
    }
}
