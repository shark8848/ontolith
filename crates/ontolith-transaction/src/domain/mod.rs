#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TxnId(pub u128);

impl TxnId {
    pub const fn new(value: u128) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxnMode {
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxnState {
    Active,
    Committed,
    Aborted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transaction {
    pub id: TxnId,
    pub mode: TxnMode,
    pub state: TxnState,
}

impl Transaction {
    pub const fn new(id: TxnId, mode: TxnMode) -> Self {
        Self {
            id,
            mode,
            state: TxnState::Active,
        }
    }

    pub const fn is_active(&self) -> bool {
        matches!(self.state, TxnState::Active)
    }
}

pub fn status() -> &'static str {
    "domain"
}
