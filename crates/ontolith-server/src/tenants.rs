//! RocksDB-backed tenant registry (tenant management).
//!
//! Persists tenant records and the API-key digest index in the storage
//! engine's dedicated `tenant` column family, accessed only through the
//! `tenant_cf_*` byte-level primitives. Key layout:
//!   `t:<id>`        → tenant JSON (see `tenant_to_json`)
//!   `k:<digest>`    → owning tenant id (API-key digest index)

use ontolith_core::error::OntolithError;
use ontolith_security::application::TenantStore;
use ontolith_security::domain::{Tenant, tenant_from_json, tenant_to_json};
use ontolith_storage::infrastructure::{RocksDbStorageEngine, TenantCfOp};
use std::sync::Arc;

fn tenant_key(id: &str) -> Vec<u8> {
    format!("t:{id}").into_bytes()
}

fn digest_key(digest: &str) -> Vec<u8> {
    format!("k:{digest}").into_bytes()
}

/// Durable tenant registry over the `tenant` column family.
pub struct RocksTenantStore {
    engine: Arc<RocksDbStorageEngine>,
}

impl RocksTenantStore {
    pub fn new(engine: Arc<RocksDbStorageEngine>) -> Result<Self, OntolithError> {
        let store = Self { engine };
        // Touch the CF so a corrupt/partial registry fails at open time.
        let _ = store.list()?;
        Ok(store)
    }

    fn write_tenant(&self, tenant: &Tenant) -> Result<(), OntolithError> {
        let mut ops: Vec<TenantCfOp> = Vec::new();
        // Remove stale digest index entries for this tenant, then re-add.
        let existing = self
            .engine
            .tenant_cf_get(&tenant_key(tenant.id.as_str()))?
            .map(|v| tenant_from_json(&String::from_utf8_lossy(&v)))
            .transpose()?;
        if let Some(prev) = existing {
            for key in &prev.api_keys {
                ops.push(TenantCfOp::Delete(digest_key(&key.digest)));
            }
        }
        ops.push(TenantCfOp::Put(
            tenant_key(tenant.id.as_str()),
            tenant_to_json(tenant).into_bytes(),
        ));
        for key in &tenant.api_keys {
            ops.push(TenantCfOp::Put(
                digest_key(&key.digest),
                tenant.id.as_str().as_bytes().to_vec(),
            ));
        }
        self.engine.tenant_cf_write_batch(&ops)
    }
}

impl TenantStore for RocksTenantStore {
    fn list(&self) -> Result<Vec<Tenant>, OntolithError> {
        let mut tenants = Vec::new();
        for (key, value) in self.engine.tenant_cf_scan_prefix(b"t:")? {
            let _ = key;
            tenants.push(tenant_from_json(&String::from_utf8_lossy(&value))?);
        }
        tenants.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
        Ok(tenants)
    }

    fn get(&self, id: &str) -> Result<Option<Tenant>, OntolithError> {
        let Some(value) = self.engine.tenant_cf_get(&tenant_key(id))? else {
            return Ok(None);
        };
        Ok(Some(tenant_from_json(&String::from_utf8_lossy(&value))?))
    }

    fn put(&self, tenant: &Tenant) -> Result<(), OntolithError> {
        self.write_tenant(tenant)
    }

    fn delete(&self, id: &str) -> Result<(), OntolithError> {
        let mut ops: Vec<TenantCfOp> = Vec::new();
        if let Some(prev) = self.get(id)? {
            for key in &prev.api_keys {
                ops.push(TenantCfOp::Delete(digest_key(&key.digest)));
            }
        }
        ops.push(TenantCfOp::Delete(tenant_key(id)));
        self.engine.tenant_cf_write_batch(&ops)
    }

    fn resolve_key(&self, raw_key: &str) -> Result<Option<Tenant>, OntolithError> {
        let digest = ontolith_security::infrastructure::api_key_digest(raw_key);
        let Some(id) = self.engine.tenant_cf_get(&digest_key(&digest))? else {
            return Ok(None);
        };
        let id = String::from_utf8_lossy(&id).into_owned();
        self.get(&id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ontolith_security::application::TenantService;
    use ontolith_security::domain::TenantStatus;
    use std::sync::Arc;

    #[test]
    fn rocks_tenant_store_persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let (raw, tenant_id);
        {
            let engine = Arc::new(RocksDbStorageEngine::open(&path).expect("open"));
            let store = Arc::new(RocksTenantStore::new(Arc::clone(&engine)).expect("store"));
            let svc = TenantService::new(store);
            let (_, k) = svc
                .create("acme", "Acme", "demo", TenantStatus::Active, true, 1)
                .expect("create");
            raw = k.expect("key");
            tenant_id = "acme".to_owned();
            assert!(svc.store().resolve_key(&raw).unwrap().is_some());
        }
        {
            let engine = Arc::new(RocksDbStorageEngine::open(&path).expect("reopen"));
            let store = Arc::new(RocksTenantStore::new(Arc::clone(&engine)).expect("store"));
            let tenant = store.get(&tenant_id).unwrap().expect("tenant survives");
            assert_eq!(tenant.name, "Acme");
            assert!(store.resolve_key(&raw).unwrap().is_some());
            // Unknown key resolves to none.
            assert!(store.resolve_key("ontk_00000000").unwrap().is_none());
        }
    }
}
