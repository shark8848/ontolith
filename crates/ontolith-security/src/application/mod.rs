//! Security application services (L5).

use crate::domain::{AuditEvent, AuditOutcome, AuthContext, AuthMode, Permission, TenantId};
use crate::infrastructure::FileAuditLog;
use ontolith_core::domain::TimestampMs;
use ontolith_core::error::OntolithError;
use std::sync::Mutex;

/// Extract / build auth context from transport headers.
pub trait Authenticator: Send + Sync {
    fn authenticate(
        &self,
        tenant: Option<&str>,
        user: Option<&str>,
        api_key: Option<&str>,
    ) -> Result<AuthContext, OntolithError>;
}

/// Simple header/API-key authenticator for R1 baseline.
#[derive(Debug, Clone)]
pub struct HeaderAuthenticator {
    pub mode: AuthMode,
    /// When set, `api_key` must match in Enforced mode (demo secret).
    pub api_key: Option<String>,
    /// Default permissions granted to authenticated tenants.
    pub default_permissions: Vec<Permission>,
}

impl Default for HeaderAuthenticator {
    fn default() -> Self {
        Self {
            mode: AuthMode::Disabled,
            api_key: None,
            default_permissions: vec![
                Permission::new("sparql", "query"),
                Permission::new("sparql", "explain"),
                Permission::new("metrics", "read"),
                Permission::new("health", "read"),
                Permission::new("data", "write"),
                Permission::new("cluster", "admin"),
            ],
        }
    }
}

impl Authenticator for HeaderAuthenticator {
    fn authenticate(
        &self,
        tenant: Option<&str>,
        user: Option<&str>,
        api_key: Option<&str>,
    ) -> Result<AuthContext, OntolithError> {
        match self.mode {
            AuthMode::Disabled => Ok(AuthContext::system_admin()),
            AuthMode::Enforced => {
                if let Some(expected) = &self.api_key {
                    match api_key {
                        Some(k) if k == expected => {}
                        _ => {
                            return Err(OntolithError::Failed(
                                "unauthorized: invalid or missing api key".into(),
                            ));
                        }
                    }
                }
                let tenant = tenant
                    .filter(|t| !t.is_empty())
                    .ok_or_else(|| OntolithError::Failed("unauthorized: missing tenant".into()))?;
                let user = user
                    .filter(|u| !u.is_empty())
                    .ok_or_else(|| OntolithError::Failed("unauthorized: missing user".into()))?;
                Ok(AuthContext::tenant_user(
                    tenant,
                    user,
                    self.default_permissions.clone(),
                ))
            }
        }
    }
}

/// In-memory audit log (append-only for process lifetime), with optional
/// durable JSONL mirror via [`FileAuditLog`].
#[derive(Debug, Default)]
pub struct InMemoryAuditLog {
    events: Mutex<Vec<AuditEvent>>,
    file: Option<FileAuditLog>,
}

impl InMemoryAuditLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach a durable file sink. Existing in-memory events are not rewritten.
    pub fn with_file_sink(mut self, file: FileAuditLog) -> Self {
        self.file = Some(file);
        self
    }

    pub fn set_file_sink(&mut self, file: FileAuditLog) {
        self.file = Some(file);
    }

    pub fn file_path(&self) -> Option<String> {
        self.file.as_ref().map(|f| f.path().display().to_string())
    }

    pub fn record(
        &self,
        timestamp_ms: TimestampMs,
        ctx: &AuthContext,
        action: impl Into<String>,
        resource: impl Into<String>,
        outcome: AuditOutcome,
        detail: impl Into<String>,
    ) {
        let event = AuditEvent {
            timestamp_ms,
            tenant: ctx.tenant.as_str().to_owned(),
            user: ctx.user.as_str().to_owned(),
            action: action.into(),
            resource: resource.into(),
            outcome,
            detail: detail.into(),
        };
        if let Some(file) = &self.file {
            // Best-effort durable mirror; memory path remains primary for process queries.
            let _ = file.append(&event);
        }
        if let Ok(mut guard) = self.events.lock() {
            guard.push(event);
        }
    }

    pub fn events(&self) -> Vec<AuditEvent> {
        // Prefer merged view: durable history + in-memory tail when file present.
        if let Some(file) = &self.file
            && let Ok(disk) = file.load_tail(10_000)
            && !disk.is_empty()
        {
            return disk;
        }
        self.events.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn len(&self) -> usize {
        if let Some(file) = &self.file
            && let Ok(n) = file.len()
        {
            return n;
        }
        self.events.lock().map(|g| g.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn by_tenant(&self, tenant: &TenantId) -> Vec<AuditEvent> {
        self.events()
            .into_iter()
            .filter(|e| e.tenant == tenant.as_str())
            .collect()
    }
}

/// Enforce permission and write audit outcome.
pub fn authorize(
    audit: &InMemoryAuditLog,
    ctx: &AuthContext,
    resource: &str,
    action: &str,
    now_ms: TimestampMs,
) -> Result<(), OntolithError> {
    match ctx.require(resource, action) {
        Ok(()) => {
            audit.record(
                now_ms,
                ctx,
                action,
                resource,
                AuditOutcome::Allow,
                "authorized",
            );
            Ok(())
        }
        Err(err) => {
            audit.record(
                now_ms,
                ctx,
                action,
                resource,
                AuditOutcome::Deny,
                err.message(),
            );
            Err(err)
        }
    }
}

pub fn status() -> &'static str {
    "application"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::UserId;

    #[test]
    fn disabled_mode_returns_admin() {
        let auth = HeaderAuthenticator::default();
        let ctx = auth.authenticate(None, None, None).unwrap();
        assert_eq!(ctx.user, UserId::new("system"));
        assert!(ctx.can("anything", "goes"));
    }

    #[test]
    fn enforced_requires_tenant_user() {
        let auth = HeaderAuthenticator {
            mode: AuthMode::Enforced,
            api_key: Some("secret".into()),
            ..Default::default()
        };
        assert!(auth.authenticate(None, None, Some("secret")).is_err());
        let ctx = auth
            .authenticate(Some("acme"), Some("alice"), Some("secret"))
            .unwrap();
        assert_eq!(ctx.tenant, TenantId::new("acme"));
        assert!(ctx.can("sparql", "query"));
    }

    #[test]
    fn audit_log_records_and_filters_tenant() {
        let log = InMemoryAuditLog::new();
        let ctx = AuthContext::tenant_user("t1", "u1", vec![]);
        log.record(1, &ctx, "query", "sparql", AuditOutcome::Deny, "nope");
        assert_eq!(log.len(), 1);
        assert_eq!(log.by_tenant(&TenantId::new("t1")).len(), 1);
        assert!(log.by_tenant(&TenantId::new("other")).is_empty());
    }
}
