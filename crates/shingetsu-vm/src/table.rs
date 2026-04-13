use std::sync::Arc;

use tokio::sync::RwLock;

/// A Lua table value.  The `Arc` makes `Clone` cheap; the inner `RwLock`
/// allows concurrent readers and serialises writers.
///
/// Phase 1: tables are not yet fully implemented.  This type exists so that
/// `Value::Table` compiles; the array/hash parts are added in Phase 2.
#[derive(Clone)]
pub struct Table(pub(crate) Arc<TableState>);

pub(crate) struct TableState {
    #[allow(dead_code)]
    pub(crate) inner: RwLock<TableInner>,
}

#[allow(dead_code)]
pub(crate) struct TableInner {
    // Phase 2 will add array and hash parts.
}

impl Table {
    pub fn new() -> Self {
        Table(Arc::new(TableState {
            inner: RwLock::new(TableInner {}),
        }))
    }
}

impl Default for Table {
    fn default() -> Self {
        Self::new()
    }
}
