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

/// Minimal synchronous HTTP GET (status text, Content-Type, body). Shared by
/// the JSON-LD remote-context loader and SPARQL Update `LOAD <http://…>`.
/// `https://` is not implemented yet and returns a deterministic error.
pub(crate) fn http_get(url: &str, accept: &str) -> Result<(String, String, String), OntolithError> {
    if url.starts_with("https://") {
        return Err(OntolithError::Unsupported(
            "remote fetch: https:// not implemented (only http://)",
        ));
    }
    let rest = url
        .strip_prefix("http://")
        .ok_or(OntolithError::Unsupported(
            "remote fetch: only http(s):// URLs are supported",
        ))?;
    // Strip any fragment before building the request; the query string is
    // part of the request target.
    let rest = rest.split('#').next().unwrap_or(rest);
    let (host_port, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };
    if host_port.is_empty() {
        return Err(OntolithError::failed(format!(
            "remote fetch: malformed URL {url:?}"
        )));
    }
    let addr = host_port
        .to_socket_addrs()
        .ok()
        .and_then(|mut it| it.next())
        .ok_or_else(|| {
            OntolithError::Failed(format!("cannot resolve remote fetch host {host_port:?}"))
        })?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).map_err(|e| {
        OntolithError::Failed(format!("connect remote fetch host {host_port}: {e}"))
    })?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let head = format!(
        "GET {path} HTTP/1.1\r\nHost: {host_port}\r\nAccept: {accept}\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(head.as_bytes())
        .and_then(|_| stream.flush())
        .map_err(|e| OntolithError::Failed(format!("write remote fetch request: {e}")))?;

    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|e| OntolithError::Failed(format!("read remote fetch response: {e}")))?;
    let text = String::from_utf8_lossy(&raw);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or(("", &text));
    let status_line = head.lines().next().unwrap_or_default().to_owned();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_owned();
    if !status.starts_with('2') {
        return Err(OntolithError::Failed(format!(
            "remote fetch {url} returned HTTP {status}"
        )));
    }
    let content_type = head
        .lines()
        .find_map(|line| {
            let lower = line.to_ascii_lowercase();
            lower
                .strip_prefix("content-type:")
                .map(str::trim)
                .map(str::to_owned)
        })
        .unwrap_or_default();
    Ok((status_line, content_type, body.to_owned()))
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
