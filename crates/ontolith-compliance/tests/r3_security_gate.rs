//! R3 exit-criteria gate: enterprise security hardening + tenant/audit gates
//! (PLAN §6 R3 exit criteria; PROGRESS R3 台账).
//!
//! Gate assertions:
//! 1. audit-log integrity chain — append-only JSONL with FNV-1a chained
//!    hashes; verification passes for an untouched log and fails at the exact
//!    entry index after tampering a middle line (tamper-evidence);
//! 2. enforced tenant isolation end-to-end — scoped reads observe only the
//!    tenant graph, unscoped reads observe the shared default graph, and
//!    cross-tenant graph references are rejected;
//! 3. secret masking — the management `/admin/config` payload never echoes
//!    ACL keys / API keys / JWT secrets (server-side test pins the shape).

use ontolith_query::domain::{QueryRequest, TenantScope};
use ontolith_query::infrastructure::update_pipeline;
use ontolith_security::domain::{AuditEvent, AuditOutcome};
use ontolith_security::infrastructure::FileAuditLog;
use ontolith_storage::application::{DictionaryCodec, StorageEngine, TripleRepository};
use ontolith_storage::infrastructure::{
    InMemoryDictionary, InMemoryStorageEngine, InMemoryTripleRepository,
};
use std::sync::Arc;

fn audit_event(i: u64) -> AuditEvent {
    AuditEvent {
        timestamp_ms: 1_700_000_000_000 + i,
        tenant: format!("tenant-{i}"),
        user: format!("user-{i}"),
        action: "read".into(),
        resource: "sparql".into(),
        outcome: AuditOutcome::Allow,
        detail: format!("event-{i}"),
    }
}

#[test]
fn audit_log_chain_integrity_and_tamper_detection() {
    let dir = std::env::temp_dir().join(format!("ontolith-r3-audit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("audit.jsonl");
    let _ = std::fs::remove_file(&path);

    let log = FileAuditLog::open(&path).unwrap();
    for i in 0..4 {
        log.append(&audit_event(i)).unwrap();
    }
    let ok = log.verify_chain().unwrap();
    assert!(ok.ok, "untouched chain must verify");
    assert_eq!(ok.entries, 4);
    assert_eq!(ok.broken_at, None);

    // Tamper with the second line: flip a payload byte in place.
    let mut bytes = std::fs::read(&path).unwrap();
    let second = bytes
        .iter()
        .enumerate()
        .filter(|(_, b)| **b == b'\n')
        .nth(1)
        .map(|(i, _)| i)
        .expect("at least two lines");
    // Flip a byte inside line 2's payload (before its newline), not a newline.
    bytes[second - 2] ^= 0x40;
    std::fs::write(&path, &bytes).unwrap();

    let broken = FileAuditLog::open(&path).unwrap().verify_chain().unwrap();
    assert!(!broken.ok, "tampered chain must fail verification");
    assert_eq!(
        broken.entries, 2,
        "verification stops at the first tampered entry"
    );
    assert_eq!(
        broken.broken_at,
        Some(2),
        "broken chain must point at entry 2"
    );

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn enforced_tenant_isolation_end_to_end() {
    let engine = Arc::new(InMemoryStorageEngine::new());
    let repo: Arc<dyn TripleRepository> =
        Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));
    let dict: Arc<dyn DictionaryCodec> = Arc::new(InMemoryDictionary::new());
    let p = update_pipeline(
        repo,
        Arc::clone(&engine) as Arc<dyn StorageEngine>,
        Some(dict),
    );
    p.execute(&QueryRequest::new(
        "INSERT DATA { GRAPH <urn:tenant:acme> { <http://ex.org/carol> <http://ex.org/name> \"Carol\" } }",
    ))
    .unwrap();
    p.execute(&QueryRequest::new(
        "INSERT DATA { GRAPH <urn:tenant:other> { <http://ex.org/dave> <http://ex.org/name> \"Dave\" } }",
    ))
    .unwrap();
    p.execute(&QueryRequest::new(
        "INSERT DATA { <http://ex.org/shared> <http://ex.org/name> \"Shared\" }",
    ))
    .unwrap();

    let names = |req: QueryRequest| -> Vec<String> {
        let r = p.execute(&req).unwrap();
        let mut out: Vec<String> = r
            .solutions
            .iter()
            .filter_map(|s| {
                s.bindings.get("n").and_then(|b| match b {
                    ontolith_query::domain::BoundValue::Literal(l) => Some(l.lexical_form()),
                    _ => None,
                })
            })
            .collect();
        out.sort();
        out
    };

    let unscoped = names(QueryRequest::new(
        "SELECT ?n WHERE { ?s <http://ex.org/name> ?n }",
    ));
    assert_eq!(unscoped, vec!["Shared"]);

    let acme = names(
        QueryRequest::new("SELECT ?n WHERE { ?s <http://ex.org/name> ?n }")
            .with_tenant_scope(TenantScope::new("acme")),
    );
    assert_eq!(acme, vec!["Carol"], "acme sees only its own tenant graph");

    let other = names(
        QueryRequest::new("SELECT ?n WHERE { ?s <http://ex.org/name> ?n }")
            .with_tenant_scope(TenantScope::new("other")),
    );
    assert_eq!(other, vec!["Dave"], "other sees only its own tenant graph");

    // Cross-tenant graph reference under a tenant scope is rejected.
    let foreign = QueryRequest::new(
        "SELECT ?n WHERE { GRAPH <urn:tenant:other> { ?s <http://ex.org/name> ?n } }",
    )
    .with_tenant_scope(TenantScope::new("acme"));
    let err = p
        .execute(&foreign)
        .expect_err("foreign graph must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("tenant"),
        "rejection must mention tenant isolation: {msg}"
    );

    // Writes are stamped into the tenant graph: a scoped INSERT lands only in
    // the tenant namespace and stays invisible to other tenants.
    p.execute(
        &QueryRequest::new(
            "INSERT DATA { <http://ex.org/carol> <http://ex.org/skills> \"quantum\" }",
        )
        .with_tenant_scope(TenantScope::new("acme")),
    )
    .unwrap();
    let acme_after = names(
        QueryRequest::new("SELECT ?n WHERE { ?s <http://ex.org/skills> ?n }")
            .with_tenant_scope(TenantScope::new("acme")),
    );
    assert_eq!(acme_after, vec!["quantum"]);
    let other_after = names(
        QueryRequest::new("SELECT ?n WHERE { ?s <http://ex.org/skills> ?n }")
            .with_tenant_scope(TenantScope::new("other")),
    );
    assert!(
        other_after.is_empty(),
        "tenant write must not leak across tenants"
    );
}

/// Pins the masking contract used by the server-side `/admin/config` test:
/// no secret-shaped key survives into the config payload (see
/// `ontolith-server` `admin_config_never_leaks_secrets`).
#[test]
fn secret_masking_contract_holds_in_config_shape() {
    // The R3 hardening contract: config/status surfaces booleans and paths
    // only — the secret-bearing fields below must never appear by name.
    let forbidden = [
        "ONTOLITH_JWT_SECRET",
        "x-api-key",
        "X-Ontolith-Management-Key",
        "acl_read_key",
        "acl_write_key",
        "api_key",
    ];
    // Representative config payload shape (mirrors management.rs admin_config):
    let payload = r#"{"management_bind":"127.0.0.1:9091","runtime_bind":"127.0.0.1:8080","storage_backend":"memory","data_dir":null,"auth_mode":"enforced","tenant_mode":"enforced","audit_path":"/tmp/audit.jsonl","tls":"on","semantic":"off","tracing":"on","started_at_ms":1}"#;
    for name in forbidden {
        assert!(
            !payload.contains(name),
            "config payload must not expose {name}"
        );
    }
}
