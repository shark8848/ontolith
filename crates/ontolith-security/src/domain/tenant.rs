//! Tenant registry domain model (tenant management).
//!
//! A tenant is a first-class, persisted entity owning an isolated data
//! namespace (`urn:tenant:<id>`, see [`super::TenantNamespace`]) and one or
//! more API-key credentials. Only the FNV-1a 64 digest of each key is stored;
//! the raw key is returned exactly once at creation.

use ontolith_core::domain::TimestampMs;
use ontolith_core::error::OntolithError;
use serde_json::{Value, json};

use super::TenantId;

/// Tenant lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TenantStatus {
    Active,
    Disabled,
}

impl TenantStatus {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "disabled" | "inactive" | "off" | "0" => Self::Disabled,
            _ => Self::Active,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disabled => "disabled",
        }
    }
}

/// A credential bound to a tenant. `digest` is the FNV-1a 64 hex of the raw
/// key (integrity level, dependency-free); the raw key is never persisted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantApiKey {
    pub id: String,
    pub label: String,
    pub digest: String,
    pub created_at_ms: TimestampMs,
}

/// Tenant registry entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tenant {
    pub id: TenantId,
    pub name: String,
    pub description: String,
    pub status: TenantStatus,
    pub api_keys: Vec<TenantApiKey>,
    pub created_at_ms: TimestampMs,
    pub updated_at_ms: TimestampMs,
}

impl Tenant {
    pub fn is_active(&self) -> bool {
        self.status == TenantStatus::Active
    }

    pub fn has_key_digest(&self, digest: &str) -> bool {
        self.api_keys.iter().any(|k| k.digest == digest)
    }
}

/// Validate a tenant id: 1..=64 chars of `[a-z0-9_-]`; `system` is reserved
/// for the built-in admin context and cannot be managed as a tenant.
pub fn validate_tenant_id(raw: &str) -> Result<TenantId, OntolithError> {
    if raw.is_empty() || raw.len() > 64 {
        return Err(OntolithError::failed(format!(
            "tenant id must be 1..=64 chars, got {:?}",
            raw
        )));
    }
    if !raw
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    {
        return Err(OntolithError::failed(format!(
            "tenant id must match [a-z0-9_-]+: {raw:?}"
        )));
    }
    if raw == "system" {
        return Err(OntolithError::InvalidArgument(
            "tenant id 'system' is reserved",
        ));
    }
    Ok(TenantId::new(raw))
}

fn str_field(v: &Value, key: &str) -> Result<String, OntolithError> {
    v.get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| OntolithError::Failed(format!("tenant json missing {key}")))
}

fn u64_field(v: &Value, key: &str) -> Result<u64, OntolithError> {
    v.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| OntolithError::Failed(format!("tenant json missing {key}")))
}

/// Deterministic JSON encoding of a tenant (registry wire format).
pub fn tenant_to_json(tenant: &Tenant) -> String {
    let keys = tenant
        .api_keys
        .iter()
        .map(|k| {
            json!({
                "id": k.id,
                "label": k.label,
                "digest": k.digest,
                "created_at_ms": k.created_at_ms,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "id": tenant.id.as_str(),
        "name": tenant.name,
        "description": tenant.description,
        "status": tenant.status.as_str(),
        "api_keys": keys,
        "created_at_ms": tenant.created_at_ms,
        "updated_at_ms": tenant.updated_at_ms,
    })
    .to_string()
}

/// Parse a tenant from its registry JSON.
pub fn tenant_from_json(raw: &str) -> Result<Tenant, OntolithError> {
    let v: Value = serde_json::from_str(raw)
        .map_err(|e| OntolithError::Failed(format!("tenant json parse: {e}")))?;
    let id = validate_tenant_id(str_field(&v, "id")?.as_str())?;
    let status = TenantStatus::parse(str_field(&v, "status")?.as_str());
    let keys = v
        .get("api_keys")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|k| {
            Ok(TenantApiKey {
                id: str_field(&k, "id")?,
                label: str_field(&k, "label")?,
                digest: str_field(&k, "digest")?,
                created_at_ms: u64_field(&k, "created_at_ms")?,
            })
        })
        .collect::<Result<Vec<_>, OntolithError>>()?;
    Ok(Tenant {
        id,
        name: str_field(&v, "name")?,
        description: str_field(&v, "description")?,
        status,
        api_keys: keys,
        created_at_ms: u64_field(&v, "created_at_ms")?,
        updated_at_ms: u64_field(&v, "updated_at_ms")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Tenant {
        Tenant {
            id: TenantId::new("acme"),
            name: "Acme".into(),
            description: "demo tenant".into(),
            status: TenantStatus::Active,
            api_keys: vec![TenantApiKey {
                id: "key_1".into(),
                label: "default".into(),
                digest: "abc123".into(),
                created_at_ms: 1,
            }],
            created_at_ms: 1,
            updated_at_ms: 2,
        }
    }

    #[test]
    fn tenant_json_roundtrip_is_deterministic() {
        let t = sample();
        let encoded = tenant_to_json(&t);
        let decoded = tenant_from_json(&encoded).expect("decode");
        assert_eq!(decoded, t);
        assert_eq!(tenant_to_json(&decoded), encoded);
    }

    #[test]
    fn tenant_id_validation() {
        assert!(validate_tenant_id("acme").is_ok());
        assert!(validate_tenant_id("acme-2_prod").is_ok());
        assert!(validate_tenant_id("ACME").is_err());
        assert!(validate_tenant_id("a b").is_err());
        assert!(validate_tenant_id("").is_err());
        assert!(validate_tenant_id("system").is_err());
        assert!(validate_tenant_id(&"x".repeat(65)).is_err());
    }
}
