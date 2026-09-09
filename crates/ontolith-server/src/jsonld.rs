//! JSON-LD remote `@context` loader (WBS-02 extension).
//!
//! The parser itself performs no network I/O; this module supplies the L5
//! ingest path with a minimal synchronous HTTP(S) GET loader (same in-tree
//! transport style as the OIDC JWKS fetch), enabled via
//! `ONTOLITH_JSONLD_REMOTE_CONTEXT=1`. `http://` uses a raw TCP request;
//! `https://` is served by an in-tree rustls client (ring provider, Mozilla
//! `webpki-roots` CA set) with an optional extra PEM bundle via
//! `ONTOLITH_REMOTE_FETCH_CA_BUNDLE` (internal PKI / self-signed endpoints).

use ontolith_core::error::OntolithError;
use ontolith_parser::infrastructure::RemoteContextLoader;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

/// Environment variable naming an extra PEM CA bundle trusted by the
/// outbound `https://` fetcher (remote `LOAD`, SPARQL `SERVICE`, JSON-LD
/// remote `@context`) in addition to the Mozilla `webpki-roots` set.
pub const REMOTE_FETCH_CA_BUNDLE_ENV: &str = "ONTOLITH_REMOTE_FETCH_CA_BUNDLE";

/// Whether the ingest path should resolve remote `@context` URLs
/// (`ONTOLITH_JSONLD_REMOTE_CONTEXT` = `1`/`true`).
pub fn remote_context_enabled() -> bool {
    std::env::var("ONTOLITH_JSONLD_REMOTE_CONTEXT")
        .ok()
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

/// Minimal synchronous HTTP(S) GET (status text, Content-Type, body).
/// Shared by the JSON-LD remote-context loader, SPARQL Update
/// `LOAD <http(s)://…>` and SPARQL 1.1 `SERVICE` federation. `http://` uses
/// a raw TCP request; `https://` goes through the in-tree rustls client
/// (Mozilla `webpki-roots` + optional `ONTOLITH_REMOTE_FETCH_CA_BUNDLE`).
pub(crate) fn http_get(url: &str, accept: &str) -> Result<(String, String, String), OntolithError> {
    if url.starts_with("https://") {
        let extra = extra_ca_bundle_pem()?;
        return https_get(url, accept, extra.as_deref());
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
    parse_http_response(url, &raw)
}

/// Read the optional extra CA bundle (PEM) configured via
/// [`REMOTE_FETCH_CA_BUNDLE_ENV`]; absent or empty means "webpki-roots only".
fn extra_ca_bundle_pem() -> Result<Option<Vec<u8>>, OntolithError> {
    match std::env::var(REMOTE_FETCH_CA_BUNDLE_ENV) {
        Ok(path) if !path.trim().is_empty() => {
            let pem = std::fs::read(path.trim()).map_err(|e| {
                OntolithError::Failed(format!("remote fetch: cannot read CA bundle {path}: {e}"))
            })?;
            Ok(Some(pem))
        }
        _ => Ok(None),
    }
}

/// `https://` GET over an in-tree rustls client (ring provider). Trust roots
/// are the Mozilla `webpki-roots` set plus any PEM certificates in
/// `extra_ca_pem` (internal PKI / self-signed endpoints). `extra_ca_pem` is a
/// parameter (not read from the environment here) so tests can inject roots
/// deterministically without touching process-global state.
fn https_get(
    url: &str,
    accept: &str,
    extra_ca_pem: Option<&[u8]>,
) -> Result<(String, String, String), OntolithError> {
    let rest = url
        .strip_prefix("https://")
        .ok_or(OntolithError::Unsupported(
            "remote fetch: only http(s):// URLs are supported",
        ))?;
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
    // Split an explicit `:port` (default 443); the bare host is the TLS
    // ServerName used for certificate verification.
    let (host, port) = match host_port.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => {
            let port: u16 = p.parse().map_err(|_| {
                OntolithError::failed(format!("remote fetch: malformed port in {url:?}"))
            })?;
            (h, port)
        }
        _ => (host_port, 443u16),
    };
    let addr = format!("{host}:{port}")
        .to_socket_addrs()
        .ok()
        .and_then(|mut it| it.next())
        .ok_or_else(|| {
            OntolithError::Failed(format!("cannot resolve remote fetch host {host_port:?}"))
        })?;
    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).map_err(|e| {
        OntolithError::Failed(format!("connect remote fetch host {host_port}: {e}"))
    })?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));

    let mut roots =
        rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(pem) = extra_ca_pem {
        for cert in rustls_pemfile::certs(&mut &pem[..]) {
            let cert = cert.map_err(|e| {
                OntolithError::Failed(format!("remote fetch: malformed CA bundle PEM: {e}"))
            })?;
            roots.add(cert).map_err(|e| {
                OntolithError::Failed(format!("remote fetch: cannot trust CA bundle entry: {e}"))
            })?;
        }
    }
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let server_name = rustls::pki_types::ServerName::try_from(host.to_owned()).map_err(|e| {
        OntolithError::Failed(format!(
            "remote fetch: invalid TLS server name {host:?}: {e}"
        ))
    })?;
    let conn = rustls::ClientConnection::new(Arc::new(config), server_name)
        .map_err(|e| OntolithError::Failed(format!("remote fetch: TLS client setup: {e}")))?;
    let mut tls = rustls::StreamOwned::new(conn, stream);
    let head = format!(
        "GET {path} HTTP/1.1\r\nHost: {host_port}\r\nAccept: {accept}\r\nConnection: close\r\n\r\n"
    );
    tls.write_all(head.as_bytes())
        .and_then(|_| tls.flush())
        .map_err(|e| OntolithError::Failed(format!("write remote fetch request: {e}")))?;

    // Read until the peer closes. Servers that omit `close_notify` surface as
    // `UnexpectedEof` after the last byte; keep the bytes already received
    // (same best-effort semantics as the plain-HTTP path, which likewise
    // cannot distinguish a truncated body).
    let mut raw = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        match tls.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof && !raw.is_empty() => break,
            Err(e) => {
                return Err(OntolithError::Failed(format!(
                    "read remote fetch response: {e}"
                )));
            }
        }
    }
    parse_http_response(url, &raw)
}

/// Parse a raw HTTP/1.x response into (status line, Content-Type, body) and
/// enforce a 2xx status.
fn parse_http_response(url: &str, raw: &[u8]) -> Result<(String, String, String), OntolithError> {
    let text = String::from_utf8_lossy(raw);
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

/// Minimal HTTP(S) remote context fetcher (shares the `http_get` transport:
/// raw TCP for `http://`, in-tree rustls client for `https://`).
#[derive(Debug, Clone, Default)]
pub struct HttpRemoteContextLoader;

impl RemoteContextLoader for HttpRemoteContextLoader {
    fn load(&self, url: &str) -> Result<String, OntolithError> {
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(OntolithError::Unsupported(
                "json-ld remote @context: only http(s):// URLs supported",
            ));
        }
        let (_status, _content_type, body) =
            http_get(url, "application/ld+json, application/json")?;
        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    /// Serializes every test that mutates `ONTOLITH_REMOTE_FETCH_CA_BUNDLE`.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Spawn a one-shot TLS server (self-signed `localhost` cert) answering a
    /// single GET with `body`/`content_type`, then closing with
    /// `close_notify`. Returns the bound address; the server thread exits on
    /// its own after serving.
    fn spawn_tls_server(
        cert: &rcgen::CertifiedKey,
        body: &'static str,
        content_type: &'static str,
    ) -> std::net::SocketAddr {
        let certs = vec![cert.cert.der().clone()];
        let key = rustls::pki_types::PrivateKeyDer::Pkcs8(cert.key_pair.serialize_der().into());
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .expect("tls server config");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind tls listener");
        let addr = listener.local_addr().expect("local addr");
        thread::spawn(move || {
            let (mut tcp, _) = listener.accept().expect("accept tls client");
            let mut conn =
                rustls::ServerConnection::new(Arc::new(config)).expect("server connection");
            {
                let mut tls = rustls::Stream::new(&mut conn, &mut tcp);
                let mut req = Vec::new();
                let mut buf = [0u8; 2048];
                while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                    match tls.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => req.extend_from_slice(&buf[..n]),
                        Err(_) => return,
                    }
                }
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                tls.write_all(head.as_bytes()).expect("write head");
                tls.write_all(body.as_bytes()).expect("write body");
                tls.flush().expect("flush response");
            }
            conn.send_close_notify();
            while conn.wants_write() {
                if conn.write_tls(&mut tcp).is_err() {
                    break;
                }
            }
        });
        addr
    }

    fn self_signed_localhost() -> rcgen::CertifiedKey {
        rcgen::generate_simple_self_signed(vec!["localhost".to_owned()])
            .expect("generate self-signed cert")
    }

    #[test]
    fn https_get_trusts_extra_ca_bundle() {
        let cert = self_signed_localhost();
        let addr = spawn_tls_server(
            &cert,
            "{\"name\":\"https://ex.org/name\"}",
            "application/ld+json",
        );
        let url = format!("https://localhost:{}/ctx.jsonld", addr.port());
        let (status, content_type, body) = https_get(
            &url,
            "application/ld+json",
            Some(cert.cert.pem().as_bytes()),
        )
        .expect("https get with extra root");
        assert!(status.starts_with("HTTP/1.1 200"), "got: {status}");
        assert_eq!(content_type, "application/ld+json");
        assert_eq!(body, "{\"name\":\"https://ex.org/name\"}");
    }

    #[test]
    fn https_get_rejects_untrusted_self_signed() {
        // A different self-signed cert signs the server; the extra bundle only
        // trusts an unrelated CA, so verification must fail deterministically.
        let server_cert = self_signed_localhost();
        let unrelated_ca = self_signed_localhost();
        let addr = spawn_tls_server(&server_cert, "x", "text/plain");
        let url = format!("https://localhost:{}/doc.ttl", addr.port());
        let err = https_get(
            &url,
            "text/turtle",
            Some(unrelated_ca.cert.pem().as_bytes()),
        )
        .expect_err("untrusted cert must fail");
        assert!(
            matches!(err, OntolithError::Failed(_)),
            "expected failed, got: {err:?}"
        );
    }

    #[test]
    fn https_get_rejects_non_2xx_status() {
        // Status enforcement is shared with the plain-HTTP path; the https
        // branch must apply it too (500 surfaces as Failed, not Ok).
        let cert = self_signed_localhost();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind tls listener");
        let addr = listener.local_addr().expect("local addr");
        let certs = vec![cert.cert.der().clone()];
        let key = rustls::pki_types::PrivateKeyDer::Pkcs8(cert.key_pair.serialize_der().into());
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .expect("tls server config");
        thread::spawn(move || {
            let (mut tcp, _) = listener.accept().expect("accept tls client");
            let mut conn =
                rustls::ServerConnection::new(Arc::new(config)).expect("server connection");
            {
                let mut tls = rustls::Stream::new(&mut conn, &mut tcp);
                let mut buf = [0u8; 2048];
                let mut req = Vec::new();
                while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                    match tls.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => req.extend_from_slice(&buf[..n]),
                        Err(_) => return,
                    }
                }
                tls.write_all(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 3\r\nConnection: close\r\n\r\nerr")
                    .expect("write response");
                tls.flush().expect("flush response");
            }
            conn.send_close_notify();
            while conn.wants_write() {
                if conn.write_tls(&mut tcp).is_err() {
                    break;
                }
            }
        });
        let url = format!("https://localhost:{}/doc.ttl", addr.port());
        let err = https_get(&url, "text/turtle", Some(cert.cert.pem().as_bytes()))
            .expect_err("500 must fail");
        let msg = match err {
            OntolithError::Failed(m) => m,
            other => panic!("expected failed, got: {other:?}"),
        };
        assert!(msg.contains("500"), "got: {msg}");
    }

    #[test]
    fn http_get_dispatches_https_with_env_ca_bundle() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cert = self_signed_localhost();
        let addr = spawn_tls_server(
            &cert,
            "@prefix : <https://ex.org/> . :s :p :o .",
            "text/turtle",
        );
        let dir = tempfile::tempdir().expect("tempdir");
        let bundle = dir.path().join("ca.pem");
        std::fs::write(&bundle, cert.cert.pem()).expect("write ca bundle");
        // SAFETY: env mutation is serialized by ENV_LOCK; concurrent https
        // tests use unrelated certs, so this bundle cannot flip their outcome.
        unsafe { std::env::set_var(REMOTE_FETCH_CA_BUNDLE_ENV, &bundle) };
        let url = format!("https://localhost:{}/doc.ttl", addr.port());
        let result = http_get(&url, "text/turtle");
        // SAFETY: see set_var above; ENV_LOCK held for the whole test.
        unsafe { std::env::remove_var(REMOTE_FETCH_CA_BUNDLE_ENV) };
        let (status, content_type, body) = result.expect("http_get over https");
        assert!(status.starts_with("HTTP/1.1 200"), "got: {status}");
        assert_eq!(content_type, "text/turtle");
        assert!(body.contains(":s :p :o"), "got: {body}");
    }

    #[test]
    fn remote_context_loader_fetches_https() {
        let cert = self_signed_localhost();
        let addr = spawn_tls_server(
            &cert,
            "{\"name\":\"https://ex.org/name\"}",
            "application/ld+json",
        );
        let url = format!("https://localhost:{}/ctx.jsonld", addr.port());
        let bundle = cert.cert.pem();
        // Loader routes through `http_get`, which reads the env bundle; inject
        // via the env path guarded against unrelated tests (their certs are
        // not in this bundle).
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().expect("tempdir");
        let bundle_path = dir.path().join("ca.pem");
        std::fs::write(&bundle_path, bundle).expect("write ca bundle");
        // SAFETY: see http_get_dispatches_https_with_env_ca_bundle.
        unsafe { std::env::set_var(REMOTE_FETCH_CA_BUNDLE_ENV, &bundle_path) };
        let loader = HttpRemoteContextLoader;
        let result = loader.load(&url);
        // SAFETY: see set_var above.
        unsafe { std::env::remove_var(REMOTE_FETCH_CA_BUNDLE_ENV) };
        let body = result.expect("loader over https");
        assert_eq!(body, "{\"name\":\"https://ex.org/name\"}");
    }

    #[test]
    fn remote_context_loader_rejects_non_http_scheme() {
        let loader = HttpRemoteContextLoader;
        let err = loader
            .load("file:///etc/context.jsonld")
            .expect_err("file scheme must fail");
        assert!(
            matches!(err, OntolithError::Unsupported(_)),
            "expected unsupported, got: {err:?}"
        );
    }
}
