use crate::application::TransactionManager;
use crate::domain::{Transaction, TxnId, TxnMode, TxnState};
use ontolith_core::error::OntolithError;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

#[derive(Default)]
struct TransactionState {
    txns: HashMap<TxnId, Transaction>,
    deadlines_ms: HashMap<TxnId, u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransactionMetricsSnapshot {
    pub begun: u64,
    pub begin_rejected: u64,
    pub committed: u64,
    pub aborted: u64,
    pub expired_cleaned: u64,
    pub active: usize,
}

pub struct InMemoryTransactionManager {
    next_id: AtomicU64,
    clock_ms: AtomicU64,
    max_active: usize,
    begun_count: AtomicU64,
    begin_rejected_count: AtomicU64,
    committed_count: AtomicU64,
    aborted_count: AtomicU64,
    expired_cleaned_count: AtomicU64,
    state: RwLock<TransactionState>,
}

impl InMemoryTransactionManager {
    pub fn new() -> Self {
        Self::with_max_active(1024)
    }

    pub fn with_max_active(max_active: usize) -> Self {
        Self {
            next_id: AtomicU64::new(1),
            clock_ms: AtomicU64::new(0),
            max_active,
            begun_count: AtomicU64::new(0),
            begin_rejected_count: AtomicU64::new(0),
            committed_count: AtomicU64::new(0),
            aborted_count: AtomicU64::new(0),
            expired_cleaned_count: AtomicU64::new(0),
            state: RwLock::new(TransactionState::default()),
        }
    }

    fn allocate_id(&self) -> TxnId {
        let raw = self.next_id.fetch_add(1, Ordering::SeqCst) as u128;
        TxnId::new(raw)
    }

    pub fn now_ms(&self) -> u64 {
        self.clock_ms.load(Ordering::SeqCst)
    }

    pub fn advance_clock(&self, delta_ms: u64) {
        self.clock_ms.fetch_add(delta_ms, Ordering::SeqCst);
    }

    fn begin_internal(&self, mode: TxnMode, timeout_ms: Option<u64>) -> Result<Transaction, OntolithError> {
        let mut guard = self
            .state
            .write()
            .map_err(|_| OntolithError::InvalidState("transaction state lock poisoned"))?;

        let active_count = guard
            .txns
            .values()
            .filter(|txn| txn.state == TxnState::Active)
            .count();

        if active_count >= self.max_active {
            self.begin_rejected_count.fetch_add(1, Ordering::SeqCst);
            return Err(OntolithError::InvalidState(
                "too many active transactions",
            ));
        }

        let txn = Transaction::new(self.allocate_id(), mode);
        guard.txns.insert(txn.id, txn);

        if let Some(timeout_ms) = timeout_ms {
            guard
                .deadlines_ms
                .insert(txn.id, self.now_ms().saturating_add(timeout_ms));
        }

        self.begun_count.fetch_add(1, Ordering::SeqCst);

        Ok(txn)
    }

    fn is_expired(guard: &TransactionState, txn_id: TxnId, now_ms: u64) -> bool {
        guard
            .deadlines_ms
            .get(&txn_id)
            .is_some_and(|deadline| *deadline <= now_ms)
    }
}

impl Default for InMemoryTransactionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TransactionManager for InMemoryTransactionManager {
    fn begin(&self, mode: TxnMode) -> Result<Transaction, OntolithError> {
        self.begin_internal(mode, None)
    }

    fn begin_with_timeout(
        &self,
        mode: TxnMode,
        timeout_ms: u64,
    ) -> Result<Transaction, OntolithError> {
        self.begin_internal(mode, Some(timeout_ms))
    }

    fn commit(&self, txn_id: TxnId) -> Result<Transaction, OntolithError> {
        let mut guard = self
            .state
            .write()
            .map_err(|_| OntolithError::InvalidState("transaction state lock poisoned"))?;

        let now_ms = self.now_ms();
        let expired = Self::is_expired(&guard, txn_id, now_ms);

        if expired {
            if let Some(txn) = guard.txns.get_mut(&txn_id) {
                txn.state = TxnState::Aborted;
            } else {
                return Err(OntolithError::InvalidState("transaction not found"));
            }
            guard.deadlines_ms.remove(&txn_id);
            return Err(OntolithError::InvalidState("transaction expired"));
        }

        let result = {
            let txn = guard
                .txns
                .get_mut(&txn_id)
                .ok_or(OntolithError::InvalidState("transaction not found"))?;

            match txn.state {
                TxnState::Active => {
                    txn.state = TxnState::Committed;
                    Ok(*txn)
                }
                TxnState::Committed => {
                    Err(OntolithError::InvalidState("transaction already committed"))
                }
                TxnState::Aborted => {
                    Err(OntolithError::InvalidState("transaction already aborted"))
                }
            }
        };

        if result.is_ok() {
            guard.deadlines_ms.remove(&txn_id);
            self.committed_count.fetch_add(1, Ordering::SeqCst);
        }
        result
    }

    fn abort(&self, txn_id: TxnId) -> Result<Transaction, OntolithError> {
        let mut guard = self
            .state
            .write()
            .map_err(|_| OntolithError::InvalidState("transaction state lock poisoned"))?;

        let result = {
            let txn = guard
                .txns
                .get_mut(&txn_id)
                .ok_or(OntolithError::InvalidState("transaction not found"))?;

            match txn.state {
                TxnState::Active => {
                    txn.state = TxnState::Aborted;
                    Ok(*txn)
                }
                TxnState::Committed => {
                    Err(OntolithError::InvalidState("transaction already committed"))
                }
                TxnState::Aborted => {
                    Err(OntolithError::InvalidState("transaction already aborted"))
                }
            }
        };

        if result.is_ok() {
            guard.deadlines_ms.remove(&txn_id);
            self.aborted_count.fetch_add(1, Ordering::SeqCst);
        }
        result
    }

    fn get(&self, txn_id: TxnId) -> Option<Transaction> {
        let guard = self.state.read().ok()?;
        guard.txns.get(&txn_id).copied()
    }

    fn active_count(&self) -> usize {
        let Ok(guard) = self.state.read() else {
            return 0;
        };
        guard
            .txns
            .values()
            .filter(|txn| txn.state == TxnState::Active)
            .count()
    }

    fn cleanup_expired(&self, now_ms: u64) -> Result<Vec<TxnId>, OntolithError> {
        let mut guard = self
            .state
            .write()
            .map_err(|_| OntolithError::InvalidState("transaction state lock poisoned"))?;

        let mut expired_ids = Vec::new();
        for (txn_id, deadline) in &guard.deadlines_ms {
            if *deadline <= now_ms {
                expired_ids.push(*txn_id);
            }
        }

        for txn_id in &expired_ids {
            if let Some(txn) = guard.txns.get_mut(txn_id)
                && txn.state == TxnState::Active
            {
                txn.state = TxnState::Aborted;
            }
            guard.deadlines_ms.remove(txn_id);
        }

        self.expired_cleaned_count
            .fetch_add(expired_ids.len() as u64, Ordering::SeqCst);

        Ok(expired_ids)
    }
}

impl InMemoryTransactionManager {
    pub fn metrics_snapshot(&self) -> TransactionMetricsSnapshot {
        TransactionMetricsSnapshot {
            begun: self.begun_count.load(Ordering::SeqCst),
            begin_rejected: self.begin_rejected_count.load(Ordering::SeqCst),
            committed: self.committed_count.load(Ordering::SeqCst),
            aborted: self.aborted_count.load(Ordering::SeqCst),
            expired_cleaned: self.expired_cleaned_count.load(Ordering::SeqCst),
            active: self.active_count(),
        }
    }
}

pub fn status() -> &'static str {
    "infrastructure"
}

#[cfg(test)]
mod tests {
    use super::InMemoryTransactionManager;
    use crate::application::TransactionManager;
    use crate::domain::{TxnMode, TxnState};

    #[test]
    fn begin_creates_active_transaction() {
        let manager = InMemoryTransactionManager::new();

        let txn = manager.begin(TxnMode::ReadWrite).expect("begin must succeed");

        assert_eq!(txn.state, TxnState::Active);
        assert!(txn.id.0 > 0);
    }

    #[test]
    fn commit_transitions_state() {
        let manager = InMemoryTransactionManager::new();
        let txn = manager.begin(TxnMode::ReadWrite).expect("begin must succeed");

        let committed = manager.commit(txn.id).expect("commit must succeed");

        assert_eq!(committed.state, TxnState::Committed);
        assert_eq!(manager.get(txn.id).expect("stored txn").state, TxnState::Committed);
    }

    #[test]
    fn commit_after_abort_is_rejected() {
        let manager = InMemoryTransactionManager::new();
        let txn = manager.begin(TxnMode::ReadWrite).expect("begin must succeed");

        manager.abort(txn.id).expect("abort must succeed");
        let err = manager.commit(txn.id).expect_err("commit after abort must fail");

        assert_eq!(
            err,
            ontolith_core::error::OntolithError::InvalidState("transaction already aborted")
        );
    }

    #[test]
    fn expired_transaction_is_cleaned_up() {
        let manager = InMemoryTransactionManager::new();
        let txn = manager
            .begin_with_timeout(TxnMode::ReadWrite, 100)
            .expect("begin with timeout must succeed");

        manager.advance_clock(101);
        let expired = manager
            .cleanup_expired(manager.now_ms())
            .expect("cleanup must succeed");

        assert_eq!(expired, vec![txn.id]);
        assert_eq!(manager.get(txn.id).expect("txn must still exist").state, TxnState::Aborted);
    }

    #[test]
    fn commit_rejects_expired_transaction() {
        let manager = InMemoryTransactionManager::new();
        let txn = manager
            .begin_with_timeout(TxnMode::ReadWrite, 10)
            .expect("begin with timeout must succeed");

        manager.advance_clock(11);
        let err = manager
            .commit(txn.id)
            .expect_err("commit should fail for expired transaction");

        assert_eq!(
            err,
            ontolith_core::error::OntolithError::InvalidState("transaction expired")
        );
    }

    #[test]
    fn begin_rejects_when_active_limit_reached() {
        let manager = InMemoryTransactionManager::with_max_active(1);

        let txn = manager
            .begin(TxnMode::ReadWrite)
            .expect("first begin must succeed");
        let err = manager
            .begin(TxnMode::ReadOnly)
            .expect_err("second begin should be blocked by active limit");

        assert_eq!(
            err,
            ontolith_core::error::OntolithError::InvalidState("too many active transactions")
        );
        assert_eq!(manager.active_count(), 1);

        manager
            .commit(txn.id)
            .expect("commit must release active slot");
        assert_eq!(manager.active_count(), 0);
        assert!(manager.begin(TxnMode::ReadOnly).is_ok());
    }

    #[test]
    fn metrics_snapshot_tracks_lifecycle_events() {
        let manager = InMemoryTransactionManager::with_max_active(1);

        let first = manager
            .begin_with_timeout(TxnMode::ReadWrite, 10)
            .expect("first begin must succeed");
        let _ = manager.begin(TxnMode::ReadOnly);

        manager.advance_clock(11);
        let _ = manager.cleanup_expired(manager.now_ms()).expect("cleanup must succeed");

        let second = manager
            .begin(TxnMode::ReadOnly)
            .expect("begin after cleanup must succeed");
        manager.abort(second.id).expect("abort must succeed");

        let metrics = manager.metrics_snapshot();
        assert_eq!(metrics.begun, 2);
        assert_eq!(metrics.begin_rejected, 1);
        assert_eq!(metrics.committed, 0);
        assert_eq!(metrics.aborted, 1);
        assert_eq!(metrics.expired_cleaned, 1);
        assert_eq!(metrics.active, 0);

        assert_eq!(
            manager
                .get(first.id)
                .expect("first txn should still be tracked")
                .state,
            TxnState::Aborted
        );
    }
}
