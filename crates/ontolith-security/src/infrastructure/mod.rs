//! Security infrastructure: durable audit sinks (L5).

use crate::domain::{AuditEvent, AuditOutcome};
use ontolith_core::domain::TimestampMs;
use ontolith_core::error::OntolithError;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Append-only JSONL audit log on disk.
///
/// Format (one event per line):
/// `{"ts":…,"tenant":"…","user":"…","action":"…","resource":"…","outcome":"…","detail":"…"}`
///
/// Not tamper-proof (no hash chain yet); process restart retains history.
#[derive(Debug)]
pub struct FileAuditLog {
    path: PathBuf,
    lock: Mutex<()>,
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
        Ok(Self {
            path,
            lock: Mutex::new(()),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&self, event: &AuditEvent) -> Result<(), OntolithError> {
        let line = format!(
            r#"{{"ts":{},"tenant":{},"user":{},"action":{},"resource":{},"outcome":{},"detail":{}}}"#,
            event.timestamp_ms,
            json_escape(&event.tenant),
            json_escape(&event.user),
            json_escape(&event.action),
            json_escape(&event.resource),
            json_escape(event.outcome.as_str()),
            json_escape(&event.detail),
        );
        let _guard = self
            .lock
            .lock()
            .map_err(|_| OntolithError::Failed("audit log lock poisoned".into()))?;
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
        Ok(())
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

    fn now_ms_for_test() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}
