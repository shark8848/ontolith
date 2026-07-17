use crate::domain::{Transaction, TxnId, TxnMode};
use ontolith_core::error::OntolithError;

pub trait TransactionManager: Send + Sync {
    fn begin(&self, mode: TxnMode) -> Result<Transaction, OntolithError>;

    fn begin_with_timeout(
        &self,
        mode: TxnMode,
        timeout_ms: u64,
    ) -> Result<Transaction, OntolithError> {
        let _ = (mode, timeout_ms);
        Err(OntolithError::Unsupported(
            "transaction timeout is not supported by this manager",
        ))
    }

    fn commit(&self, txn_id: TxnId) -> Result<Transaction, OntolithError>;

    fn abort(&self, txn_id: TxnId) -> Result<Transaction, OntolithError>;

    fn get(&self, txn_id: TxnId) -> Option<Transaction>;

    fn active_count(&self) -> usize {
        0
    }

    fn cleanup_expired(&self, now_ms: u64) -> Result<Vec<TxnId>, OntolithError> {
        let _ = now_ms;
        Ok(Vec::new())
    }
}

pub struct UnitOfWork<'a, M: TransactionManager> {
    manager: &'a M,
    txn: Transaction,
}

impl<'a, M: TransactionManager> UnitOfWork<'a, M> {
    pub fn begin(manager: &'a M, mode: TxnMode) -> Result<Self, OntolithError> {
        let txn = manager.begin(mode)?;
        Ok(Self { manager, txn })
    }

    pub const fn transaction(&self) -> Transaction {
        self.txn
    }

    pub fn commit(self) -> Result<Transaction, OntolithError> {
        self.manager.commit(self.txn.id)
    }

    pub fn abort(self) -> Result<Transaction, OntolithError> {
        self.manager.abort(self.txn.id)
    }
}

pub fn run_in_transaction<M, F, T>(
    manager: &M,
    mode: TxnMode,
    operation: F,
) -> Result<T, OntolithError>
where
    M: TransactionManager,
    F: FnOnce(Transaction) -> Result<T, OntolithError>,
{
    let txn = manager.begin(mode)?;
    match operation(txn) {
        Ok(value) => {
            manager.commit(txn.id)?;
            Ok(value)
        }
        Err(err) => {
            let _ = manager.abort(txn.id);
            Err(err)
        }
    }
}

pub fn status() -> &'static str {
    "application"
}
