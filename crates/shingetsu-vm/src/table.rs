use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::RwLock;

use bytes::Bytes;
use indexmap::IndexMap;

use crate::error::VmError;
use crate::gc::GcHeader;
use crate::value::Value;

/// Build the `VmError` raised when a write is attempted on a frozen
/// (`table.freeze`d) table.  Matches LuaU's wording.
fn frozen_table_error() -> VmError {
    let msg = "attempt to modify a readonly table".to_owned();
    VmError::LuaError {
        display: msg.clone(),
        value: Value::string(msg),
    }
}

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
        Value::Nil => Err(VmError::TableKeyIsNil { name: None }),
        Value::Boolean(b) => Ok(HashableValue::Boolean(*b)),
        Value::Integer(i) => Ok(HashableValue::Integer(*i)),
        Value::Float(f) => {
            if f.is_nan() {
                return Err(VmError::TableKeyIsNaN { name: None });
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
    /// GC tri-colour header.
    pub(crate) gc: GcHeader,
    /// Frozen (read-only) flag.  Set by `table.freeze`; never cleared
    /// (LuaU has no `table.unfreeze`).  Outside `RwLock` so the fast
    /// path on every mutation is a single relaxed atomic load.
    pub(crate) frozen: AtomicBool,
    /// Fast lockless check for metatable presence.  Kept in sync with
    /// `inner.metatable` by `set_metatable`.
    pub(crate) has_metatable: AtomicBool,
    pub(crate) inner: RwLock<TableInner>,
}

pub(crate) struct TableInner {
    /// One-based integer keys `1..=array.len()`.  `array[0]` is Lua key 1.
    /// Maintained without trailing `nil`s so `array.len()` equals `#t` for
    /// sequences.
    pub(crate) array: Vec<Value>,
    /// All other keys (and integer keys outside the array range), in
    /// insertion order.  Each entry stores `(original_key, value)` so that
    /// `next` / `pairs` can return the original key (including reference
    /// types such as Tables).
    pub(crate) hash: IndexMap<HashableValue, (Value, Value)>,
    /// Optional metatable.  `None` means no metatable is set.
    pub(crate) metatable: Option<Table>,
}

impl Table {
    pub fn new() -> Self {
        Table(Arc::new(TableState {
            gc: GcHeader::new(),
            frozen: AtomicBool::new(false),
            has_metatable: AtomicBool::new(false),
            inner: RwLock::new(TableInner {
                array: Vec::new(),
                hash: IndexMap::new(),
                metatable: None,
            }),
        }))
    }

    /// Return `true` if this table has been frozen via `table.freeze`.
    pub fn is_frozen(&self) -> bool {
        self.0.frozen.load(Ordering::Relaxed)
    }

    /// Mark this table as read-only.  Subsequent mutation attempts
    /// (`raw_set`, `raw_insert`, `raw_remove`, `swap_array`,
    /// `set_metatable`) return `VmError::LuaError` with the LuaU
    /// message "attempt to modify a readonly table".  Already-frozen
    /// tables remain frozen; this is a no-op in that case.
    pub fn freeze(&self) {
        self.0.frozen.store(true, Ordering::Relaxed);
    }

    /// Helper used by every mutation entry point.
    fn check_writable(&self) -> Result<(), VmError> {
        if self.is_frozen() {
            Err(frozen_table_error())
        } else {
            Ok(())
        }
    }

    /// Return a clone of this table's metatable, or `None`.
    pub fn get_metatable(&self) -> Option<Table> {
        self.0.inner.read().metatable.clone()
    }

    /// Set (or clear) this table's metatable.  Errors if the table is
    /// frozen.
    pub fn set_metatable(&self, mt: Option<Table>) -> Result<(), VmError> {
        self.check_writable()?;
        let has = mt.is_some();
        self.0.inner.write().metatable = mt;
        self.0.has_metatable.store(has, Ordering::Release);
        Ok(())
    }

    /// Fast lockless check for metatable presence.
    #[inline]
    pub fn has_metatable(&self) -> bool {
        self.0.has_metatable.load(Ordering::Relaxed)
    }

    /// Look up a metamethod by event name (e.g. `b"__index"`) in this
    /// table's metatable.  Returns `None` if there is no metatable or the
    /// event key is absent / nil.
    pub fn get_metamethod(&self, event: impl AsRef<[u8]>) -> Option<Value> {
        let inner = self.0.inner.read();
        let mt = inner.metatable.as_ref()?;
        // Avoid holding the outer read-lock while reading the metatable, to
        // prevent deadlock if the table is its own metatable.
        let mt = mt.clone();
        drop(inner);
        let key = Value::String(Bytes::copy_from_slice(event.as_ref()));
        mt.raw_get(&key).ok().filter(|v| !v.is_nil())
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
                    inner
                        .hash
                        .get(&hk)
                        .map(|(_, v)| v.clone())
                        .unwrap_or(Value::Nil)
                }
            }
            _ => inner
                .hash
                .get(&hk)
                .map(|(_, v)| v.clone())
                .unwrap_or(Value::Nil),
        })
    }

    /// Read a string-keyed field and convert via [`FromLua`].
    ///
    /// This is a convenience wrapper around [`raw_get`](Self::raw_get) that
    /// builds the string key and applies `FromLua` conversion in one step.
    /// Use `Option<T>` as the target type for optional fields.
    pub fn get_field<T: crate::convert::FromLua>(&self, key: &str) -> Result<T, VmError> {
        let v = self.raw_get(&Value::String(Bytes::copy_from_slice(key.as_bytes())))?;
        T::from_lua(v).map_err(|e| match e {
            VmError::BadArgument {
                position,
                function,
                expected,
                got,
            } => {
                let got = if got == "nil" {
                    format!("field '{}' is missing", key)
                } else {
                    got
                };
                VmError::BadArgument {
                    position,
                    function,
                    expected: format!("{} for field '{}'", expected, key),
                    got,
                }
            }
            other => other,
        })
    }

    /// Write a value by key.  Setting a key to `nil` removes it.
    /// Errors if the table is frozen.
    pub fn raw_set(&self, key: Value, val: Value) -> Result<(), VmError> {
        self.check_writable()?;
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
                        if let Some((_, v)) = inner.hash.shift_remove(&next) {
                            inner.array.push(v);
                        } else {
                            break;
                        }
                    }
                } else {
                    inner.hash.insert(hk, (key, val));
                }
            }
            _ => {
                if matches!(val, Value::Nil) {
                    inner.hash.shift_remove(&hk);
                } else {
                    inner.hash.insert(hk, (key, val));
                }
            }
        }
        Ok(())
    }

    /// Return the next key-value pair after `key` in table iteration order
    /// (array part first, then hash part in insertion order).  Pass
    /// `Value::Nil` to get the first pair.  Returns `None` when `key` was
    /// the last entry.
    pub fn next(&self, key: &Value) -> Result<Option<(Value, Value)>, VmError> {
        let inner = self.0.inner.read();

        if key.is_nil() {
            // Return the first non-nil element.
            for (i, v) in inner.array.iter().enumerate() {
                if !v.is_nil() {
                    return Ok(Some((Value::Integer(i as i64 + 1), v.clone())));
                }
            }
            if let Some((_, (orig_k, v))) = inner.hash.iter().next() {
                return Ok(Some((orig_k.clone(), v.clone())));
            }
            return Ok(None);
        }

        let hk = to_hashable(key)?;

        // Check array part first.
        if let HashableValue::Integer(i) = &hk {
            if *i >= 1 {
                let idx = (*i as usize) - 1;
                if idx < inner.array.len() {
                    // Found in array: return the next non-nil entry.
                    for j in (idx + 1)..inner.array.len() {
                        if !inner.array[j].is_nil() {
                            return Ok(Some((
                                Value::Integer(j as i64 + 1),
                                inner.array[j].clone(),
                            )));
                        }
                    }
                    // Past end of array: return first hash entry.
                    if let Some((_, (orig_k, v))) = inner.hash.iter().next() {
                        return Ok(Some((orig_k.clone(), v.clone())));
                    }
                    return Ok(None);
                }
            }
        }

        // Search hash part.
        let mut found = false;
        for (k, (orig_k, v)) in inner.hash.iter() {
            if found {
                return Ok(Some((orig_k.clone(), v.clone())));
            }
            if k == &hk {
                found = true;
            }
        }
        Ok(None) // key was last, or not found
    }

    /// The Lua length operator `#t`.  Returns the length of the sequence part
    /// (array length after trailing-nil trimming).
    pub fn raw_len(&self) -> i64 {
        self.0.inner.read().array.len() as i64
    }

    /// Insert `val` at 1-based position `pos` in the sequence part,
    /// shifting elements up.  `pos` must be in `[1, #t+1]`.  Errors
    /// if the table is frozen.
    pub fn raw_insert(&self, pos: usize, val: Value) -> Result<(), VmError> {
        self.check_writable()?;
        let mut inner = self.0.inner.write();
        // pos is 1-based; convert to 0-based index.
        let idx = pos - 1;
        if idx >= inner.array.len() {
            // Appending at end (or beyond).
            inner.array.push(val);
        } else {
            inner.array.insert(idx, val);
        }
        Ok(())
    }

    /// Remove and return the element at 1-based position `pos` in the
    /// sequence part, shifting elements down.  Returns `Value::Nil` if
    /// the position is out of range.  Errors if the table is frozen.
    pub fn raw_remove(&self, pos: usize) -> Result<Value, VmError> {
        self.check_writable()?;
        let mut inner = self.0.inner.write();
        let idx = pos - 1;
        if idx < inner.array.len() {
            let val = inner.array.remove(idx);
            // Trim trailing nils to maintain the sequence invariant.
            while matches!(inner.array.last(), Some(Value::Nil)) {
                inner.array.pop();
            }
            Ok(val)
        } else {
            Ok(Value::Nil)
        }
    }

    /// Swap the array (sequence) part with `arr`, returning the previous
    /// contents.  Call with an empty `Vec` to take the array out, or with
    /// a sorted/modified `Vec` to put it back.  Errors if the table is
    /// frozen.
    pub fn swap_array(&self, arr: &mut Vec<Value>) -> Result<(), VmError> {
        self.check_writable()?;
        std::mem::swap(&mut self.0.inner.write().array, arr);
        Ok(())
    }

    /// Clear every entry (both array and hash parts).  Preserves the
    /// metatable and backing storage capacity.  Errors if the table is
    /// frozen.  Used by `table.clear`.
    pub fn raw_clear(&self) -> Result<(), VmError> {
        self.check_writable()?;
        let mut inner = self.0.inner.write();
        inner.array.clear();
        inner.hash.clear();
        Ok(())
    }

    /// Return a shallow copy of this table: same keys, values, and
    /// metatable; the clone is not frozen even if `self` is.  Used by
    /// `table.clone`.
    pub fn raw_clone(&self) -> Table {
        let inner = self.0.inner.read();
        let copy = Table::new();
        {
            let mut dst = copy.0.inner.write();
            dst.array = inner.array.clone();
            dst.hash = inner.hash.clone();
            let has_mt = inner.metatable.is_some();
            dst.metatable = inner.metatable.clone();
            copy.0.has_metatable.store(has_mt, Ordering::Release);
        }
        copy
    }

    /// Length respecting `__len` metamethod.  Delegates to
    /// [`CallContext::table_len`].
    pub async fn len(
        &self,
        ctx: &crate::call_context::CallContext,
    ) -> Result<i64, crate::error::VmError> {
        ctx.table_len(self).await
    }

    /// Read by key respecting `__index` metamethod.  Delegates to
    /// [`CallContext::table_get`].
    pub async fn get(
        &self,
        key: &Value,
        ctx: &crate::call_context::CallContext,
    ) -> Result<Value, crate::error::VmError> {
        ctx.table_get(self, key).await
    }

    /// Write by key respecting `__newindex` metamethod.  Delegates to
    /// [`CallContext::table_set`].
    pub async fn set(
        &self,
        key: Value,
        value: Value,
        ctx: &crate::call_context::CallContext,
    ) -> Result<(), crate::error::VmError> {
        ctx.table_set(self, key, value).await
    }
}

impl Default for Table {
    fn default() -> Self {
        Self::new()
    }
}
