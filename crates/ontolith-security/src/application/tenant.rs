//! Tenant registry application services (tenant management).
//!
//! [`TenantStore`] is the persistence boundary (in-memory by default; the
//! gateway wires a RocksDB-backed implementation over the storage engine's
//! dedicated `tenant` column family). [`TenantService`] adds validation,
//! key generation and lifecycle rules on top.

use crate::domain::{Tenant, TenantApiKey, TenantStatus, validate_tenant_id};
use crate::infrastructure::{api_key_digest, generate_api_key};
use ontolith_core::domain::TimestampMs;
use ontolith_core::error::OntolithError;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Persistence boundary for the tenant registry.
pub trait TenantStore: Send + Sync {
    fn list(&self) -> Result<Vec<Tenant>, OntolithError>;
    fn get(&self, id: &str) -> Result<Option<Tenant>, OntolithError>;
    fn put(&self, tenant: &Tenant) -> Result<(), OntolithError>;
    fn delete(&self, id: &str) -> Result<(), OntolithError>;
    /// Resolve a raw API key to its owning tenant (by digest).
    fn resolve_key(&self, raw_key: &str) -> Result<Option<Tenant>, OntolithError>;
}

/// In-memory tenant registry (single-node default; not durable).
#[derive(Debug, Default)]
pub struct MemoryTenantStore {
    inner: Mutex<MemoryInner>,
}

#[derive(Debug, Default)]
struct MemoryInner {
    tenants: HashMap<String, Tenant>,
    by_digest: HashMap<String, String>,
}

impl MemoryTenantStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn reindex(inner: &mut MemoryInner, tenant: &Tenant) {
        inner
            .by_digest
            .retain(|_, owner| owner != tenant.id.as_str());
        for key in &tenant.api_keys {
            inner
                .by_digest
                .insert(key.digest.clone(), tenant.id.as_str().to_owned());
        }
    }
}

impl TenantStore for MemoryTenantStore {
    fn list(&self) -> Result<Vec<Tenant>, OntolithError> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| OntolithError::Failed("tenant store lock poisoned".into()))?;
        let mut tenants = inner.tenants.values().cloned().collect::<Vec<_>>();
        tenants.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
        Ok(tenants)
    }

    fn get(&self, id: &str) -> Result<Option<Tenant>, OntolithError> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| OntolithError::Failed("tenant store lock poisoned".into()))?;
        Ok(inner.tenants.get(id).cloned())
    }

    fn put(&self, tenant: &Tenant) -> Result<(), OntolithError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| OntolithError::Failed("tenant store lock poisoned".into()))?;
        inner
            .tenants
            .insert(tenant.id.as_str().to_owned(), tenant.clone());
        Self::reindex(&mut inner, tenant);
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<(), OntolithError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| OntolithError::Failed("tenant store lock poisoned".into()))?;
        inner.tenants.remove(id);
        inner.by_digest.retain(|_, owner| owner != id);
        Ok(())
    }

    fn resolve_key(&self, raw_key: &str) -> Result<Option<Tenant>, OntolithError> {
        let digest = api_key_digest(raw_key);
        let inner = self
            .inner
            .lock()
            .map_err(|_| OntolithError::Failed("tenant store lock poisoned".into()))?;
        let Some(owner) = inner.by_digest.get(&digest) else {
            return Ok(None);
        };
        Ok(inner.tenants.get(owner).cloned())
    }
}

/// Tenant lifecycle service: validation + key management on top of a store.
pub struct TenantService {
    store: Arc<dyn TenantStore>,
    key_counter: AtomicU64,
}

impl TenantService {
    pub fn new(store: Arc<dyn TenantStore>) -> Self {
        Self {
            store,
            key_counter: AtomicU64::new(1),
        }
    }

    pub fn store(&self) -> &Arc<dyn TenantStore> {
        &self.store
    }

    pub fn list(&self) -> Result<Vec<Tenant>, OntolithError> {
        self.store.list()
    }

    pub fn get(&self, id: &str) -> Result<Option<Tenant>, OntolithError> {
        self.store.get(id)
    }

    /// Create a tenant. When `generate_key` is set, returns the raw API key
    /// (shown exactly once; only its digest is stored).
    pub fn create(
        &self,
        id: &str,
        name: &str,
        description: &str,
        status: TenantStatus,
        generate_key: bool,
        now_ms: TimestampMs,
    ) -> Result<(Tenant, Option<String>), OntolithError> {
        let tenant_id = validate_tenant_id(id)?;
        if self.store.get(id)?.is_some() {
            return Err(OntolithError::failed(format!(
                "tenant already exists: {id}"
            )));
        }
        let mut tenant = Tenant {
            id: tenant_id,
            name: name.trim().to_owned(),
            description: description.trim().to_owned(),
            status,
            api_keys: Vec::new(),
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        };
        if tenant.name.is_empty() {
            tenant.name = id.to_owned();
        }
        let raw = if generate_key {
            let (key, raw) = self.new_key(&mut tenant, "default", now_ms)?;
            tenant.api_keys.push(key);
            Some(raw)
        } else {
            None
        };
        self.store.put(&tenant)?;
        Ok((tenant, raw))
    }

    /// Update name / description / status (id is immutable).
    pub fn update(
        &self,
        id: &str,
        name: &str,
        description: &str,
        status: TenantStatus,
        now_ms: TimestampMs,
    ) -> Result<Tenant, OntolithError> {
        let mut tenant = self
            .store
            .get(id)?
            .ok_or(OntolithError::NotFound("tenant not found"))?;
        tenant.name = name.trim().to_owned();
        tenant.description = description.trim().to_owned();
        tenant.status = status;
        tenant.updated_at_ms = now_ms;
        self.store.put(&tenant)?;
        Ok(tenant)
    }

    pub fn delete(&self, id: &str) -> Result<(), OntolithError> {
        validate_tenant_id(id)?;
        self.store.delete(id)
    }

    /// Add a key to a tenant; returns `(key_id, raw_key)` (raw shown once).
    pub fn add_key(
        &self,
        id: &str,
        label: &str,
        now_ms: TimestampMs,
    ) -> Result<(String, String), OntolithError> {
        let mut tenant = self
            .store
            .get(id)?
            .ok_or(OntolithError::NotFound("tenant not found"))?;
        let (key, raw) = self.new_key(&mut tenant, label, now_ms)?;
        tenant.api_keys.push(key);
        tenant.updated_at_ms = now_ms;
        self.store.put(&tenant)?;
        Ok((tenant.api_keys.last().expect("just pushed").id.clone(), raw))
    }

    /// Revoke a key by id.
    pub fn revoke_key(
        &self,
        id: &str,
        key_id: &str,
        now_ms: TimestampMs,
    ) -> Result<Tenant, OntolithError> {
        let mut tenant = self
            .store
            .get(id)?
            .ok_or(OntolithError::NotFound("tenant not found"))?;
        let before = tenant.api_keys.len();
        tenant.api_keys.retain(|k| k.id != key_id);
        if tenant.api_keys.len() == before {
            return Err(OntolithError::NotFound("tenant key not found"));
        }
        tenant.updated_at_ms = now_ms;
        self.store.put(&tenant)?;
        Ok(tenant)
    }

    fn new_key(
        &self,
        tenant: &mut Tenant,
        label: &str,
        now_ms: TimestampMs,
    ) -> Result<(TenantApiKey, String), OntolithError> {
        let counter = self.key_counter.fetch_add(1, Ordering::Relaxed);
        let raw = generate_api_key(tenant.id.as_str(), now_ms, counter);
        // The key id is the hex digest suffix (raw is shown exactly once).
        let key = TenantApiKey {
            id: format!("k_{}", &raw[raw.len() - 16..]),
            label: label.trim().to_owned(),
            digest: api_key_digest(&raw),
            created_at_ms: now_ms,
        };
        Ok((key, raw))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_crud_and_key_resolution() {
        let store = Arc::new(MemoryTenantStore::new()) as Arc<dyn TenantStore>;
        let svc = TenantService::new(store);
        let (t, raw) = svc
            .create("acme", "Acme", "demo", TenantStatus::Active, true, 1)
            .expect("create");
        assert_eq!(t.id.as_str(), "acme");
        let raw = raw.expect("generated key");
        assert!(raw.starts_with("ontk_"));

        let resolved = svc.store().resolve_key(&raw).expect("resolve");
        assert_eq!(resolved.unwrap().id.as_str(), "acme");
        assert!(svc.store().resolve_key("ontk_0000").unwrap().is_none());

        let (kid, raw2) = svc.add_key("acme", "ops", 2).expect("add key");
        assert_ne!(raw, raw2);
        assert!(kid.starts_with("k_"));
        assert_eq!(
            svc.store().resolve_key(&raw2).unwrap().unwrap().id.as_str(),
            "acme"
        );

        svc.revoke_key("acme", &kid, 3).expect("revoke");
        assert!(svc.store().resolve_key(&raw2).unwrap().is_none());

        svc.update("acme", "Acme Inc", "x", TenantStatus::Disabled, 4)
            .expect("update");
        assert_eq!(
            svc.get("acme").unwrap().unwrap().status,
            TenantStatus::Disabled
        );

        svc.delete("acme").expect("delete");
        assert!(svc.get("acme").unwrap().is_none());
        assert!(svc.store().resolve_key(&raw).unwrap().is_none());
    }

    #[test]
    fn create_rejects_duplicate_and_invalid_ids() {
        let store = Arc::new(MemoryTenantStore::new()) as Arc<dyn TenantStore>;
        let svc = TenantService::new(store);
        svc.create("acme", "Acme", "", TenantStatus::Active, false, 1)
            .expect("create");
        assert!(
            svc.create("acme", "x", "", TenantStatus::Active, false, 2)
                .is_err()
        );
        assert!(
            svc.create("SYSTEM", "x", "", TenantStatus::Active, false, 2)
                .is_err()
        );
        assert!(
            svc.create("bad id", "x", "", TenantStatus::Active, false, 2)
                .is_err()
        );
    }

    #[test]
    fn generated_keys_are_unique_within_a_process() {
        let store = Arc::new(MemoryTenantStore::new()) as Arc<dyn TenantStore>;
        let svc = TenantService::new(store);
        let mut raws = std::collections::HashSet::new();
        for i in 0..50 {
            let (_, raw) = svc
                .create(&format!("t{i}"), "", "", TenantStatus::Active, true, 1000)
                .expect("create");
            assert!(raws.insert(raw.unwrap()));
        }
    }
}
