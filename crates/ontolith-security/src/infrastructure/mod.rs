//! Security infrastructure: durable audit sinks (L5).

use crate::domain::{AuditEvent, AuditOutcome};
use ontolith_core::domain::TimestampMs;
use ontolith_core::error::OntolithError;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub mod jwt;
pub use jwt::{
    JwtClaims, JwtVerifyOptions, auth_context_from_claims, sign_hs256, sign_tenant_token,
    verify_hs256,
};
pub mod oidc;
pub use oidc::{
    CachingJwks, Jwk, Jwks, JwksFetcher, JwksVerifier, OidcConfig, OidcDiscovery, RsaPublicKey,
    verify_oidc_token,
};

/// Append-only JSONL audit log on disk with an integrity hash chain.
///
/// Format (one event per line):
/// `{"ts":…,"tenant":"…","user":"…","action":"…","resource":"…","outcome":"…","detail":"…","prev":"<hex>","hash":"<hex>"}`
///
/// Each entry chains the previous entry's hash (genesis = 0) with the event
/// payload using FNV-1a 64. This is an integrity-level chain (dependency-free,
/// deterministic); a cryptographic upgrade keeps the same schema.
#[derive(Debug)]
pub struct FileAuditLog {
    path: PathBuf,
    lock: Mutex<ChainState>,
}

#[derive(Debug, Default)]
struct ChainState {
    last_hash: u64,
}

/// Result of a full-chain integrity verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainVerify {
    pub ok: bool,
    pub entries: usize,
    /// 1-based index of the first broken entry, if any.
    pub broken_at: Option<usize>,
}

impl FileAuditLog {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, OntolithError> {
        let path = path.into();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|e| {
                OntolithError::Failed(format!("audit log create_dir {}: {e}", parent.display()))
            })?;
        }
        // Touch file so reopen always succeeds.
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| {
                OntolithError::Failed(format!("audit log open {}: {e}", path.display()))
            })?;
        let last_hash = last_written_hash(&path)?;
        Ok(Self {
            path,
            lock: Mutex::new(ChainState { last_hash }),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&self, event: &AuditEvent) -> Result<(), OntolithError> {
        let mut guard = self
            .lock
            .lock()
            .map_err(|_| OntolithError::Failed("audit log lock poisoned".into()))?;
        let prev = guard.last_hash;
        let payload = audit_fields_json(
            event.timestamp_ms,
            &event.tenant,
            &event.user,
            &event.action,
            &event.resource,
            event.outcome.as_str(),
            &event.detail,
        );
        let hash = chain_hash(prev, payload.as_bytes());
        let line = format!(
            r#"{{"ts":{},"tenant":{},"user":{},"action":{},"resource":{},"outcome":{},"detail":{},"prev":"{prev:016x}","hash":"{hash:016x}"}}"#,
            event.timestamp_ms,
            json_escape(&event.tenant),
            json_escape(&event.user),
            json_escape(&event.action),
            json_escape(&event.resource),
            json_escape(event.outcome.as_str()),
            json_escape(&event.detail),
        );
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| {
                OntolithError::Failed(format!(
                    "audit log append open {}: {e}",
                    self.path.display()
                ))
            })?;
        writeln!(f, "{line}").map_err(|e| {
            OntolithError::Failed(format!("audit log write {}: {e}", self.path.display()))
        })?;
        f.flush().map_err(|e| {
            OntolithError::Failed(format!("audit log flush {}: {e}", self.path.display()))
        })?;
        guard.last_hash = hash;
        Ok(())
    }

    /// Re-verify the full hash chain from genesis.
    pub fn verify_chain(&self) -> Result<ChainVerify, OntolithError> {
        let file = File::open(&self.path).map_err(|e| {
            OntolithError::Failed(format!("audit log read {}: {e}", self.path.display()))
        })?;
        let reader = BufReader::new(file);
        let mut expected_prev = 0u64;
        let mut entries = 0usize;
        for line in reader.lines() {
            let line = line.map_err(|e| {
                OntolithError::Failed(format!("audit log readline {}: {e}", self.path.display()))
            })?;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            entries += 1;
            let Some(meta) = parse_jsonl_chain_meta(line) else {
                return Ok(ChainVerify {
                    ok: false,
                    entries,
                    broken_at: Some(entries),
                });
            };
            if meta.prev != expected_prev || meta.hash != chain_hash(expected_prev, &meta.payload) {
                return Ok(ChainVerify {
                    ok: false,
                    entries,
                    broken_at: Some(entries),
                });
            }
            expected_prev = meta.hash;
        }
        Ok(ChainVerify {
            ok: true,
            entries,
            broken_at: None,
        })
    }

    pub fn load_tail(&self, limit: usize) -> Result<Vec<AuditEvent>, OntolithError> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| OntolithError::Failed("audit log lock poisoned".into()))?;
        let file = File::open(&self.path).map_err(|e| {
            OntolithError::Failed(format!("audit log read {}: {e}", self.path.display()))
        })?;
        let reader = BufReader::new(file);
        let mut all = Vec::new();
        for line in reader.lines() {
            let line = line.map_err(|e| {
                OntolithError::Failed(format!("audit log readline {}: {e}", self.path.display()))
            })?;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(ev) = parse_jsonl_event(line) {
                all.push(ev);
            }
        }
        if limit == 0 || all.len() <= limit {
            Ok(all)
        } else {
            Ok(all.split_off(all.len() - limit))
        }
    }

    pub fn len(&self) -> Result<usize, OntolithError> {
        Ok(self.load_tail(usize::MAX)?.len())
    }

    pub fn is_empty(&self) -> Result<bool, OntolithError> {
        Ok(self.len()? == 0)
    }
}

fn audit_fields_json(
    ts: u64,
    tenant: &str,
    user: &str,
    action: &str,
    resource: &str,
    outcome: &str,
    detail: &str,
) -> String {
    format!(
        r#"{{"ts":{},"tenant":{},"user":{},"action":{},"resource":{},"outcome":{},"detail":{}}}"#,
        ts,
        json_escape(tenant),
        json_escape(user),
        json_escape(action),
        json_escape(resource),
        json_escape(outcome),
        json_escape(detail),
    )
}

/// FNV-1a 64-bit (deterministic, dependency-free; integrity level only).
pub(crate) fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Hex digest of a raw API key (FNV-1a 64, 16 hex chars). Keys are stored
/// only by digest in the tenant registry; the raw value is shown once at
/// creation.
pub fn api_key_digest(raw: &str) -> String {
    format!("{:016x}", fnv1a64(raw.as_bytes()))
}

/// Generate a tenant-bound API key: `ontk_<16 hex>`. Deterministic per
/// (tenant, now, counter) triple; the counter keeps same-millisecond calls
/// distinct within a process.
pub fn generate_api_key(tenant: &str, now_ms: u64, counter: u64) -> String {
    let mut buf = Vec::with_capacity(tenant.len() + 16);
    buf.extend_from_slice(tenant.as_bytes());
    buf.extend_from_slice(&now_ms.to_le_bytes());
    buf.extend_from_slice(&counter.to_le_bytes());
    format!("ontk_{:016x}", fnv1a64(&buf))
}

fn chain_hash(prev: u64, payload: &[u8]) -> u64 {
    let mut chain = Vec::with_capacity(8 + payload.len());
    chain.extend_from_slice(&prev.to_le_bytes());
    chain.extend_from_slice(payload);
    fnv1a64(&chain)
}

struct ChainMeta {
    prev: u64,
    hash: u64,
    payload: Vec<u8>,
}

fn parse_jsonl_chain_meta(line: &str) -> Option<ChainMeta> {
    let prev = parse_hex_u64(extract_string(line, "\"prev\"")?.as_str())?;
    let hash = parse_hex_u64(extract_string(line, "\"hash\"")?.as_str())?;
    let ts = extract_number(line, "\"ts\"")?;
    let tenant = extract_string(line, "\"tenant\"")?;
    let user = extract_string(line, "\"user\"")?;
    let action = extract_string(line, "\"action\"")?;
    let resource = extract_string(line, "\"resource\"")?;
    let outcome = extract_string(line, "\"outcome\"")?;
    let detail = extract_string(line, "\"detail\"").unwrap_or_default();
    let payload =
        audit_fields_json(ts, &tenant, &user, &action, &resource, &outcome, &detail).into_bytes();
    Some(ChainMeta {
        prev,
        hash,
        payload,
    })
}

fn parse_hex_u64(hex: &str) -> Option<u64> {
    u64::from_str_radix(hex.trim(), 16).ok()
}

fn last_written_hash(path: &Path) -> Result<u64, OntolithError> {
    let file = File::open(path)
        .map_err(|e| OntolithError::Failed(format!("audit log read {}: {e}", path.display())))?;
    let reader = BufReader::new(file);
    let mut last = 0u64;
    for line in reader.lines() {
        let line = line.map_err(|e| {
            OntolithError::Failed(format!("audit log readline {}: {e}", path.display()))
        })?;
        if let Some(meta) = parse_jsonl_chain_meta(line.trim()) {
            last = meta.hash;
        }
    }
    Ok(last)
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn parse_jsonl_event(line: &str) -> Option<AuditEvent> {
    // Minimal field extraction without a JSON crate.
    let ts = extract_number(line, "\"ts\"")?;
    let tenant = extract_string(line, "\"tenant\"")?;
    let user = extract_string(line, "\"user\"")?;
    let action = extract_string(line, "\"action\"")?;
    let resource = extract_string(line, "\"resource\"")?;
    let outcome_s = extract_string(line, "\"outcome\"")?;
    let detail = extract_string(line, "\"detail\"").unwrap_or_default();
    let outcome = match outcome_s.as_str() {
        "allow" => AuditOutcome::Allow,
        "deny" => AuditOutcome::Deny,
        "error" => AuditOutcome::Error,
        _ => return None,
    };
    Some(AuditEvent {
        timestamp_ms: ts as TimestampMs,
        tenant,
        user,
        action,
        resource,
        outcome,
        detail,
    })
}

fn extract_number(line: &str, key: &str) -> Option<u64> {
    let idx = line.find(key)?;
    let rest = &line[idx + key.len()..];
    let rest = rest.trim_start_matches(|c: char| c == ':' || c.is_whitespace());
    let mut n = 0u64;
    let mut any = false;
    for ch in rest.chars() {
        if let Some(d) = ch.to_digit(10) {
            any = true;
            n = n.saturating_mul(10).saturating_add(d as u64);
        } else if any {
            break;
        } else {
            return None;
        }
    }
    any.then_some(n)
}

fn extract_string(line: &str, key: &str) -> Option<String> {
    let idx = line.find(key)?;
    let rest = &line[idx + key.len()..];
    let rest = rest.trim_start_matches(|c: char| c == ':' || c.is_whitespace());
    if !rest.starts_with('"') {
        return None;
    }
    let mut out = String::new();
    let mut chars = rest[1..].chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => match chars.next()? {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'u' => {
                    let mut hex = String::new();
                    for _ in 0..4 {
                        hex.push(chars.next()?);
                    }
                    if let Ok(v) = u32::from_str_radix(&hex, 16)
                        && let Some(c) = char::from_u32(v)
                    {
                        out.push(c);
                    }
                }
                other => out.push(other),
            },
            '"' => return Some(out),
            c => out.push(c),
        }
    }
    None
}

pub fn status() -> &'static str {
    "infrastructure"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::InMemoryAuditLog;
    use crate::domain::AuthContext;

    #[test]
    fn file_audit_survives_reopen() {
        let dir = std::env::temp_dir().join(format!(
            "ontolith-audit-{}-{}",
            std::process::id(),
            now_ms_for_test()
        ));
        let path = dir.join("audit.jsonl");
        {
            let log = FileAuditLog::open(&path).expect("open");
            let ctx = AuthContext::tenant_user("acme", "alice", vec![]);
            log.append(&AuditEvent {
                timestamp_ms: 1,
                tenant: ctx.tenant.as_str().into(),
                user: ctx.user.as_str().into(),
                action: "query".into(),
                resource: "sparql".into(),
                outcome: AuditOutcome::Allow,
                detail: "ok".into(),
            })
            .unwrap();
            log.append(&AuditEvent {
                timestamp_ms: 2,
                tenant: "acme".into(),
                user: "bob".into(),
                action: "write".into(),
                resource: "data".into(),
                outcome: AuditOutcome::Deny,
                detail: "nope".into(),
            })
            .unwrap();
            assert_eq!(log.len().unwrap(), 2);
            assert!(!log.is_empty().unwrap());
        }
        let log = FileAuditLog::open(&path).expect("reopen");
        let events = log.load_tail(10).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].user, "alice");
        assert_eq!(events[1].outcome, AuditOutcome::Deny);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn in_memory_still_available() {
        let mem = InMemoryAuditLog::new();
        assert!(mem.is_empty());
    }

    #[test]
    fn audit_hash_chain_verifies_and_survives_reopen() {
        let dir = std::env::temp_dir().join(format!(
            "ontolith-audit-chain-{}-{}",
            std::process::id(),
            now_ms_for_test()
        ));
        let path = dir.join("audit.jsonl");
        {
            let log = FileAuditLog::open(&path).expect("open");
            for i in 0..3u64 {
                log.append(&AuditEvent {
                    timestamp_ms: i,
                    tenant: "acme".into(),
                    user: "alice".into(),
                    action: "query".into(),
                    resource: "sparql".into(),
                    outcome: AuditOutcome::Allow,
                    detail: format!("evt-{i}"),
                })
                .unwrap();
            }
            let verify = log.verify_chain().expect("verify");
            assert!(verify.ok);
            assert_eq!(verify.entries, 3);
            assert_eq!(verify.broken_at, None);
        }
        // Reopen continues the chain from the recovered tail hash.
        let log = FileAuditLog::open(&path).expect("reopen");
        log.append(&AuditEvent {
            timestamp_ms: 99,
            tenant: "acme".into(),
            user: "bob".into(),
            action: "write".into(),
            resource: "data".into(),
            outcome: AuditOutcome::Deny,
            detail: "late".into(),
        })
        .unwrap();
        let verify = log.verify_chain().expect("verify after reopen");
        assert!(verify.ok);
        assert_eq!(verify.entries, 4);
        assert_eq!(log.len().unwrap(), 4);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn audit_hash_chain_detects_tampering() {
        let dir = std::env::temp_dir().join(format!(
            "ontolith-audit-tamper-{}-{}",
            std::process::id(),
            now_ms_for_test()
        ));
        let path = dir.join("audit.jsonl");
        let log = FileAuditLog::open(&path).expect("open");
        log.append(&AuditEvent {
            timestamp_ms: 1,
            tenant: "acme".into(),
            user: "alice".into(),
            action: "query".into(),
            resource: "sparql".into(),
            outcome: AuditOutcome::Allow,
            detail: "ok".into(),
        })
        .unwrap();
        log.append(&AuditEvent {
            timestamp_ms: 2,
            tenant: "acme".into(),
            user: "bob".into(),
            action: "write".into(),
            resource: "data".into(),
            outcome: AuditOutcome::Deny,
            detail: "nope".into(),
        })
        .unwrap();
        assert!(log.verify_chain().expect("verify").ok);

        // Flip a payload byte in the first line; chain must break.
        let raw = std::fs::read(&path).expect("read raw");
        let mut tampered = raw.clone();
        let payload_start = tampered
            .windows(6)
            .position(|w| w == b"\"acme\"")
            .expect("tenant payload");
        tampered[payload_start + 2] ^= 0x01; // 'c' -> 'b'
        std::fs::write(&path, &tampered).expect("write tampered");

        let verify = log.verify_chain().expect("verify tampered");
        assert!(!verify.ok);
        assert_eq!(verify.broken_at, Some(1));
        let _ = std::fs::remove_dir_all(dir);
    }

    fn now_ms_for_test() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}
