//! Security application services (L5).

use crate::domain::{
    AuditEvent, AuditOutcome, AuthContext, AuthMode, Permission, TenantId, TenantStatus,
};
use crate::infrastructure::{
    FileAuditLog, JwksVerifier, JwtVerifyOptions, OidcConfig, auth_context_from_claims,
    verify_hs256, verify_oidc_token,
};
use ontolith_core::domain::TimestampMs;
use ontolith_core::error::OntolithError;
use std::fmt;
use std::sync::{Arc, Mutex};

pub mod tenant;
pub use tenant::{MemoryTenantStore, TenantService, TenantStore};

/// Extract / build auth context from transport headers.
pub trait Authenticator: Send + Sync {
    fn authenticate(
        &self,
        tenant: Option<&str>,
        user: Option<&str>,
        api_key: Option<&str>,
    ) -> Result<AuthContext, OntolithError>;

    /// Authenticate with an optional `Authorization: Bearer <jwt>` token.
    /// Defaults to the header/API-key path when no bearer token is handled.
    fn authenticate_with_bearer(
        &self,
        tenant: Option<&str>,
        user: Option<&str>,
        api_key: Option<&str>,
        bearer: Option<&str>,
    ) -> Result<AuthContext, OntolithError> {
        let _ = bearer;
        self.authenticate(tenant, user, api_key)
    }
}

/// Simple header/API-key authenticator for R1 baseline.
#[derive(Clone)]
pub struct HeaderAuthenticator {
    pub mode: AuthMode,
    /// When set, `api_key` must match in Enforced mode (demo secret).
    pub api_key: Option<String>,
    /// When set, `Authorization: Bearer <HS256 jwt>` is accepted in Enforced
    /// mode as an alternative to API-key + tenant/user headers (P5-02).
    pub jwt_secret: Option<String>,
    /// Optional `iss` claim policy for JWT verification.
    pub jwt_issuer: Option<String>,
    /// Optional `aud` claim policy for JWT verification.
    pub jwt_audience: Option<String>,
    /// Optional OIDC JWKS verification: when set, `Authorization: Bearer`
    /// tokens are verified against the key set (RS256/HS256 + exp/nbf/iss/aud,
    /// TTL-refreshed through the injected fetcher) instead of the
    /// shared-secret HS256 path. Enables the R2+ OIDC chain.
    pub jwt_oidc: Option<JwksVerifier>,
    /// Clock leeway (seconds) for JWKS-verified token `exp`/`nbf`.
    pub jwt_leeway_secs: u64,
    /// Default permissions granted to authenticated tenants.
    pub default_permissions: Vec<Permission>,
    /// Optional tenant registry (tenant management): when present and
    /// non-empty, API keys resolve to their owning tenant and the global
    /// `api_key` becomes a legacy fallback. Disabled tenants are rejected.
    pub tenants: Option<Arc<dyn TenantStore>>,
}

impl fmt::Debug for HeaderAuthenticator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HeaderAuthenticator")
            .field("mode", &self.mode)
            .field("api_key", &self.api_key.as_deref().map(|_| "[redacted]"))
            .field(
                "jwt_secret",
                &self.jwt_secret.as_deref().map(|_| "[redacted]"),
            )
            .field("jwt_issuer", &self.jwt_issuer)
            .field("jwt_audience", &self.jwt_audience)
            .field("jwt_oidc", &self.jwt_oidc)
            .field("jwt_leeway_secs", &self.jwt_leeway_secs)
            .field("default_permissions", &self.default_permissions)
            .field("tenants", &self.tenants.as_ref().map(|_| "[tenant-store]"))
            .finish()
    }
}

impl Default for HeaderAuthenticator {
    fn default() -> Self {
        Self {
            mode: AuthMode::Disabled,
            api_key: None,
            jwt_secret: None,
            jwt_issuer: None,
            jwt_audience: None,
            jwt_oidc: None,
            jwt_leeway_secs: 0,
            default_permissions: vec![
                Permission::new("sparql", "query"),
                Permission::new("sparql", "explain"),
                Permission::new("metrics", "read"),
                Permission::new("health", "read"),
                Permission::new("data", "write"),
                Permission::new("cluster", "admin"),
            ],
            tenants: None,
        }
    }
}

impl HeaderAuthenticator {
    /// True when any JWT/Bearer verification path is configured: the
    /// shared-secret HS256 path (P5-02) or the OIDC JWKS chain (R2+).
    pub fn jwt_enabled(&self) -> bool {
        self.jwt_secret.is_some() || self.jwt_oidc.is_some()
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
                // Per-tenant API keys (tenant management): resolve the key to
                // its owning tenant; the tenant is taken from the registry,
                // and the optional `x-ontolith-tenant` header (if present)
                // must match. Disabled tenants are rejected.
                if let Some(store) = &self.tenants
                    && let Some(raw) = api_key.filter(|k| !k.is_empty())
                    && let Ok(Some(rec)) = store.resolve_key(raw)
                {
                    if rec.status != TenantStatus::Active {
                        return Err(OntolithError::Failed(format!(
                            "unauthorized: tenant {} is disabled",
                            rec.id.as_str()
                        )));
                    }
                    if let Some(hint) = tenant.filter(|h| !h.is_empty())
                        && hint != rec.id.as_str()
                    {
                        return Err(OntolithError::Failed(format!(
                            "forbidden: api key belongs to tenant {}, header says {hint}",
                            rec.id.as_str()
                        )));
                    }
                    let user = user.filter(|u| !u.is_empty()).unwrap_or("api").to_owned();
                    return Ok(AuthContext::tenant_user(
                        rec.id.as_str(),
                        user,
                        self.default_permissions.clone(),
                    ));
                }
                // Legacy path: global API key + tenant/user headers.
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

    fn authenticate_with_bearer(
        &self,
        tenant: Option<&str>,
        user: Option<&str>,
        api_key: Option<&str>,
        bearer: Option<&str>,
    ) -> Result<AuthContext, OntolithError> {
        if let Some(bearer) = bearer {
            let token = bearer
                .strip_prefix("Bearer ")
                .or_else(|| bearer.strip_prefix("bearer "))
                .filter(|t| !t.is_empty());
            if let Some(token) = token {
                // OIDC/JWKS path takes precedence when configured (R2+ chain).
                if let Some(verifier) = &self.jwt_oidc {
                    let jwks = verifier.jwks()?;
                    let claims = verify_oidc_token(
                        token,
                        &jwks,
                        &OidcConfig {
                            issuer: self.jwt_issuer.clone(),
                            audience: self.jwt_audience.clone(),
                            leeway_secs: self.jwt_leeway_secs,
                        },
                    )?;
                    return Ok(auth_context_from_claims(
                        &claims,
                        tenant,
                        user,
                        self.default_permissions.clone(),
                    ));
                }
                if let Some(secret) = &self.jwt_secret {
                    let claims = verify_hs256(
                        token,
                        secret,
                        &JwtVerifyOptions {
                            issuer: self.jwt_issuer.clone(),
                            audience: self.jwt_audience.clone(),
                        },
                    )?;
                    return Ok(auth_context_from_claims(
                        &claims,
                        tenant,
                        user,
                        self.default_permissions.clone(),
                    ));
                }
            }
        }
        self.authenticate(tenant, user, api_key)
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
    fn bearer_jwt_authenticates_and_rejects_forged_tokens() {
        use crate::infrastructure::{sign_hs256, sign_tenant_token};
        let auth = HeaderAuthenticator {
            mode: AuthMode::Enforced,
            jwt_secret: Some("s3cret".into()),
            jwt_issuer: Some("ontolith".into()),
            jwt_audience: Some("ontolith-server".into()),
            ..Default::default()
        };
        let token = sign_tenant_token(
            "acme",
            "alice",
            "s3cret",
            "ontolith",
            "ontolith-server",
            300,
        )
        .unwrap();
        let ctx = auth
            .authenticate_with_bearer(None, None, None, Some(&format!("Bearer {token}")))
            .unwrap();
        assert_eq!(ctx.tenant, TenantId::new("acme"));
        assert_eq!(ctx.user, UserId::new("alice"));
        assert!(ctx.can("sparql", "query"));

        let mut claims = serde_json::Map::new();
        claims.insert("sub".into(), serde_json::Value::from("alice"));
        claims.insert("tenant".into(), serde_json::Value::from("acme"));
        let forged = sign_hs256(&claims, "wrong", Some(300)).unwrap();
        assert!(
            auth.authenticate_with_bearer(None, None, None, Some(&format!("Bearer {forged}")))
                .is_err()
        );
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
