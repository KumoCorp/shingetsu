use std::sync::Arc;

use parking_lot::RwLock;

use bytes::Bytes;
use indexmap::IndexMap;

use crate::{error::VmError, value::Value};

// ---------------------------------------------------------------------------
// HashableValue — table key type
// ---------------------------------------------------------------------------

/// A `Value` that can be used as a table key.
///
/// Nil and NaN cannot be keys.  Integer-valued floats are normalised to their
/// integer equivalent so that `t[1]` and `t[1.0]` address the same entry.
/// Reference types (Table, Function, Userdata) use pointer identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum HashableValue {
    Boolean(bool),
    Integer(i64),
    /// Non-integer-valued float stored as its bit pattern (NaN excluded).
    Float(u64),
    String(Bytes),
    /// Arc data-pointer address of a table allocation.
    TablePtr(usize),
    /// Arc data-pointer address of a function allocation.
    FunctionPtr(usize),
    /// Arc data-pointer address of a userdata allocation.
    UserdataPtr(usize),
}

/// Convert a `Value` into a `HashableValue`, returning an error for nil or NaN.
pub(crate) fn to_hashable(v: &Value) -> Result<HashableValue, VmError> {
    match v {
        Value::Nil => Err(VmError::TableKeyIsNil),
        Value::Boolean(b) => Ok(HashableValue::Boolean(*b)),
        Value::Integer(i) => Ok(HashableValue::Integer(*i)),
        Value::Float(f) => {
            if f.is_nan() {
                return Err(VmError::TableKeyIsNaN);
            }
            // Integer-valued floats coerce to the equivalent integer key.
            let i = *f as i64;
            if i as f64 == *f {
                Ok(HashableValue::Integer(i))
            } else {
                Ok(HashableValue::Float(f.to_bits()))
            }
        }
        Value::String(s) => Ok(HashableValue::String(s.clone())),
        Value::Table(t) => Ok(HashableValue::TablePtr(Arc::as_ptr(&t.0) as usize)),
        Value::Function(f) => Ok(HashableValue::FunctionPtr(Arc::as_ptr(&f.0) as usize)),
        Value::Userdata(u) => {
            // Fat-pointer → thin-pointer cast: extracts the data address.
            let addr = Arc::as_ptr(u) as *const () as usize;
            Ok(HashableValue::UserdataPtr(addr))
        }
    }
}

// ---------------------------------------------------------------------------
// Table
// ---------------------------------------------------------------------------

/// A Lua table value.  The `Arc` makes `Clone` cheap (`O(1)`); the inner
/// `RwLock` allows concurrent readers and serialises writers.
#[derive(Clone)]
pub struct Table(pub(crate) Arc<TableState>);

pub(crate) struct TableState {
    pub(crate) inner: RwLock<TableInner>,
}

pub(crate) struct TableInner {
    /// One-based integer keys `1..=array.len()`.  `array[0]` is Lua key 1.
    /// Maintained without trailing `nil`s so `array.len()` equals `#t` for
    /// sequences.
    pub(crate) array: Vec<Value>,
    /// All other keys (and integer keys outside the array range), in
    /// insertion order.
    pub(crate) hash: IndexMap<HashableValue, Value>,
}

impl Table {
    pub fn new() -> Self {
        Table(Arc::new(TableState {
            inner: RwLock::new(TableInner {
                array: Vec::new(),
                hash: IndexMap::new(),
            }),
        }))
    }

    /// Read a value by key.  Returns `Value::Nil` for absent keys.
    pub fn raw_get(&self, key: &Value) -> Result<Value, VmError> {
        let hk = to_hashable(key)?;
        let inner = self.0.inner.read();
        Ok(match &hk {
            HashableValue::Integer(i) if *i >= 1 => {
                let idx = (*i as usize) - 1;
                if idx < inner.array.len() {
                    inner.array[idx].clone()
                } else {
                    // Key is beyond the array sequence; may be in the hash part.
                    inner.hash.get(&hk).cloned().unwrap_or(Value::Nil)
                }
            }
            _ => inner.hash.get(&hk).cloned().unwrap_or(Value::Nil),
        })
    }

    /// Write a value by key.  Setting a key to `nil` removes it.
    pub fn raw_set(&self, key: Value, val: Value) -> Result<(), VmError> {
        let hk = to_hashable(&key)?;
        let mut inner = self.0.inner.write();
        match &hk {
            HashableValue::Integer(i) if *i >= 1 => {
                let idx = (*i as usize) - 1;
                if matches!(val, Value::Nil) {
                    if idx < inner.array.len() {
                        inner.array[idx] = Value::Nil;
                        // Trim trailing nils to maintain the sequence invariant.
                        while matches!(inner.array.last(), Some(Value::Nil)) {
                            inner.array.pop();
                        }
                    } else {
                        inner.hash.shift_remove(&hk);
                    }
                } else if idx < inner.array.len() {
                    inner.array[idx] = val;
                } else if idx == inner.array.len() {
                    // Extend the array sequence.
                    inner.array.push(val);
                    // Absorb any consecutive integer keys waiting in the hash.
                    loop {
                        let next = HashableValue::Integer(inner.array.len() as i64 + 1);
                        if let Some(v) = inner.hash.shift_remove(&next) {
                            inner.array.push(v);
                        } else {
                            break;
                        }
                    }
                } else {
                    inner.hash.insert(hk, val);
                }
            }
            _ => {
                if matches!(val, Value::Nil) {
                    inner.hash.shift_remove(&hk);
                } else {
                    inner.hash.insert(hk, val);
                }
            }
        }
        Ok(())
    }

    /// The Lua length operator `#t`.  Returns the length of the sequence part
    /// (array length after trailing-nil trimming).
    pub fn raw_len(&self) -> i64 {
        self.0.inner.read().array.len() as i64
    }
}

impl Default for Table {
    fn default() -> Self {
        Self::new()
    }
}
