//! Security domain model (L5).

use ontolith_core::domain::TimestampMs;
use ontolith_core::error::OntolithError;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TenantId(pub String);

impl TenantId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UserId(pub String);

impl UserId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RoleId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Permission {
    pub resource: String,
    pub action: String,
}

impl Permission {
    pub fn new(resource: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            resource: resource.into(),
            action: action.into(),
        }
    }
}

/// Authenticated request context (deny-by-default when permissions empty under enforce mode).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthContext {
    pub tenant: TenantId,
    pub user: UserId,
    pub roles: Vec<RoleId>,
    pub permissions: Vec<Permission>,
}

impl AuthContext {
    pub fn can(&self, resource: &str, action: &str) -> bool {
        self.permissions
            .iter()
            .any(|p| p.resource == resource && (p.action == action || p.action == "*"))
            || self
                .permissions
                .iter()
                .any(|p| p.resource == "*" && p.action == "*")
    }

    pub fn require(&self, resource: &str, action: &str) -> Result<(), OntolithError> {
        if self.can(resource, action) {
            Ok(())
        } else {
            Err(OntolithError::Failed(format!(
                "forbidden: {resource}:{action} for tenant={} user={}",
                self.tenant.as_str(),
                self.user.as_str()
            )))
        }
    }

    /// Anonymous/system context used when auth enforcement is disabled.
    pub fn system_admin() -> Self {
        Self {
            tenant: TenantId::new("system"),
            user: UserId::new("system"),
            roles: vec![RoleId("admin".into())],
            permissions: vec![Permission::new("*", "*")],
        }
    }

    pub fn tenant_user(
        tenant: impl Into<String>,
        user: impl Into<String>,
        permissions: Vec<Permission>,
    ) -> Self {
        Self {
            tenant: TenantId::new(tenant),
            user: UserId::new(user),
            roles: Vec::new(),
            permissions,
        }
    }
}

/// Immutable audit event (queryable log entry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEvent {
    pub timestamp_ms: TimestampMs,
    pub tenant: String,
    pub user: String,
    pub action: String,
    pub resource: String,
    pub outcome: AuditOutcome,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOutcome {
    Allow,
    Deny,
    Error,
}

impl AuditOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
            Self::Error => "error",
        }
    }
}

/// How authentication is applied on the request path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AuthMode {
    /// No credentials required; inject system admin context.
    #[default]
    Disabled,
    /// Require tenant/user headers; permissions checked.
    Enforced,
}

impl AuthMode {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "on" | "true" | "1" | "enforced" | "enforce" => Self::Enforced,
            _ => Self::Disabled,
        }
    }
}

pub fn status() -> &'static str {
    "domain"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_by_default_without_permission() {
        let ctx = AuthContext::tenant_user("t1", "u1", vec![]);
        assert!(!ctx.can("sparql", "query"));
        assert!(ctx.require("sparql", "query").is_err());
    }

    #[test]
    fn wildcard_permission_allows() {
        let ctx = AuthContext::system_admin();
        assert!(ctx.can("sparql", "query"));
    }
}
