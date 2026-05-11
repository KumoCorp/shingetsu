//! Read-only userdata proxies that surface [`SnapshotValue::Map`] and
//! [`SnapshotValue::Vec`] to Lua without eagerly rebuilding the whole
//! tree.
//!
//! Property access (`t.key`, `t[i]`) goes through `__index`, which
//! looks up a single [`SnapshotValue`] in the underlying `Arc<...>`
//! and rebuilds it on the fly.  Nested `Map` / `Vec` values return
//! another proxy, so laziness propagates down the access path.
//!
//! The proxies are *frozen* by construction: `__newindex` raises
//! pointing at a free function (`task.materialize`) that callers
//! use to obtain a mutable plain-Lua table copy.  Operations that
//! bypass metamethods (`next`, `rawget`, `rawset`, `rawlen`, parts
//! of the `table` library that use raw access) do not work on the
//! proxy; users who need them must materialize first.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use indexmap::IndexMap;

use crate::call_context::CallContext;
use crate::error::VmError;
use crate::function::Function;
use crate::global_env::GlobalEnv;
use crate::snapshot_value::{MapKey, SnapshotValue};
use crate::types::LuaType;
use crate::userdata::{PrettyShape, Userdata};
use crate::value::{Value, ValueVec};
use crate::valuevec;

const FROZEN_WRITE_MSG: &str = "attempt to modify a snapshot table";
const FROZEN_WRITE_HINT: &str =
    "snapshot tables are read-only; pass through `task.materialize(...)` to obtain a mutable copy";

fn frozen_write_error() -> VmError {
    VmError::LuaError {
        display: FROZEN_WRITE_MSG.to_owned(),
        value: Value::string(FROZEN_WRITE_MSG),
    }
    .with_hint(FROZEN_WRITE_HINT)
}

// ---------------------------------------------------------------------------
// LuaSnapshotMap
// ---------------------------------------------------------------------------

/// Read-only userdata proxy for [`SnapshotValue::Map`].
pub struct LuaSnapshotMap {
    pub(crate) inner: Arc<IndexMap<MapKey, SnapshotValue>>,
}

impl LuaSnapshotMap {
    /// Wrap an inner snapshot map without copying.
    pub fn new(inner: Arc<IndexMap<MapKey, SnapshotValue>>) -> Arc<Self> {
        Arc::new(Self { inner })
    }

    /// Walk the proxy's subtree and produce a fresh mutable Lua
    /// table in `env`.  This is the path used by `task.materialize`.
    pub fn materialize(&self, env: &GlobalEnv) -> Result<Value, VmError> {
        SnapshotValue::Map(self.inner.clone()).rebuild(env)
    }
}

#[async_trait]
impl Userdata for LuaSnapshotMap {
    fn type_name(&self) -> &'static str {
        "snapshot_map"
    }

    fn lua_type_info(&self) -> LuaType {
        LuaType::named("snapshot_map")
    }

    fn has_metamethod(&self, name: &str) -> bool {
        matches!(
            name,
            "__index" | "__newindex" | "__len" | "__tostring" | "__pairs" | "__ipairs"
        )
    }

    fn pretty_entries<'a>(&'a self, env: &GlobalEnv) -> Option<Result<PrettyShape<'a>, VmError>> {
        let env = env.clone();
        let iter = self.inner.iter().map(move |(k, v)| {
            let key = match k {
                MapKey::Integer(n) => Value::Integer(*n),
                MapKey::String(s) => Value::String(s.clone()),
            };
            Ok::<_, VmError>((key, v.rebuild_lazy(&env)?))
        });
        Some(Ok(PrettyShape::Map(Box::new(iter))))
    }

    fn index(&self, key: &Value) -> Option<Result<ValueVec, VmError>> {
        let mk = match key {
            Value::Integer(n) => MapKey::Integer(*n),
            Value::String(s) => MapKey::String(s.clone()),
            // Non-integer/string keys can never be in the map.
            _ => return Some(Ok(valuevec![Value::Nil])),
        };
        match self.inner.get(&mk) {
            None => Some(Ok(valuevec![Value::Nil])),
            // Nested-table values need a GlobalEnv to materialize, so
            // fall through to the async dispatch path.  All other
            // variants can be rebuilt without env via the sync path
            // below... except they may still need env (Snapshot
            // variant rebuilds with env, opted-in userdata).  Take
            // the simple route: defer all cases to dispatch when env
            // might be required.
            Some(SnapshotValue::Map(_))
            | Some(SnapshotValue::Vec(_))
            | Some(SnapshotValue::Snapshot(_)) => None,
            Some(v) => Some(v.rebuild_no_env().map(|val| valuevec![val])),
        }
    }

    fn newindex(&self, _key: &Value, _value: &Value) -> Option<Result<ValueVec, VmError>> {
        Some(Err(frozen_write_error()))
    }

    async fn dispatch(
        self: Arc<Self>,
        ctx: CallContext,
        metamethod: &str,
        args: ValueVec,
    ) -> Result<ValueVec, VmError> {
        match metamethod {
            "__index" => {
                // Async fallback when the sync `index` path returned
                // None (nested containers / userdata snapshots).
                let key = args.get(1).cloned().unwrap_or(Value::Nil);
                let mk = match &key {
                    Value::Integer(n) => MapKey::Integer(*n),
                    Value::String(s) => MapKey::String(s.clone()),
                    _ => return Ok(valuevec![Value::Nil]),
                };
                match self.inner.get(&mk) {
                    None => Ok(valuevec![Value::Nil]),
                    Some(sv) => Ok(valuevec![sv.rebuild_lazy(&ctx.global)?]),
                }
            }
            "__newindex" => Err(frozen_write_error()),
            "__len" => Ok(valuevec![Value::Integer(self.inner.len() as i64)]),
            "__tostring" => Ok(valuevec![Value::string(format!(
                "<snapshot_map: {} entries>",
                self.inner.len()
            ))]),
            "__pairs" => Ok(self.build_pairs_triple(ctx.global.clone())),
            "__ipairs" => Ok(self.build_ipairs_triple(ctx.global.clone())),
            other => Err(VmError::HostError {
                name: "snapshot_map".to_owned(),
                source: format!("metamethod {other} not implemented on snapshot_map").into(),
            }),
        }
    }
}

impl LuaSnapshotMap {
    /// Build the `(iter_fn, state, control)` triple for `__pairs`.
    ///
    /// The iterator is closure-based: it ignores the state and
    /// control arguments Lua threads through generic-for and carries
    /// its own monotonic position counter.  Each step calls
    /// `IndexMap::get_index(pos)` (O(1)) and rebuilds the value
    /// lazily in the captured env.
    fn build_pairs_triple(&self, env: GlobalEnv) -> ValueVec {
        let inner = self.inner.clone();
        let pos = Arc::new(AtomicUsize::new(0));
        let iter_fn = Function::wrap(
            "snapshot_map_pairs",
            move || -> Result<(Value, Value), VmError> {
                let i = pos.fetch_add(1, Ordering::SeqCst);
                match inner.get_index(i) {
                    None => Ok((Value::Nil, Value::Nil)),
                    Some((k, v)) => Ok((map_key_to_value(k), v.rebuild_lazy(&env)?)),
                }
            },
        );
        valuevec![Value::Function(iter_fn), Value::Nil, Value::Nil]
    }

    /// Build the `(iter_fn, state, control)` triple for `__ipairs`.
    ///
    /// Walks consecutive integer keys starting at 1 until a miss,
    /// matching the Lua semantic for `ipairs` on a table.  Common
    /// case for a string-keyed snapshot map: terminates immediately.
    fn build_ipairs_triple(&self, env: GlobalEnv) -> ValueVec {
        let inner = self.inner.clone();
        let next = Arc::new(AtomicUsize::new(1));
        let iter_fn = Function::wrap(
            "snapshot_map_ipairs",
            move || -> Result<(Value, Value), VmError> {
                let i = next.fetch_add(1, Ordering::SeqCst);
                match inner.get(&MapKey::Integer(i as i64)) {
                    None => Ok((Value::Nil, Value::Nil)),
                    Some(v) => Ok((Value::Integer(i as i64), v.rebuild_lazy(&env)?)),
                }
            },
        );
        valuevec![Value::Function(iter_fn), Value::Nil, Value::Nil]
    }
}

fn map_key_to_value(k: &MapKey) -> Value {
    match k {
        MapKey::Integer(n) => Value::Integer(*n),
        MapKey::String(s) => Value::String(s.clone()),
    }
}

// ---------------------------------------------------------------------------
// LuaSnapshotVec
// ---------------------------------------------------------------------------

/// Read-only userdata proxy for [`SnapshotValue::Vec`].
pub struct LuaSnapshotVec {
    pub(crate) inner: Arc<Vec<SnapshotValue>>,
}

impl LuaSnapshotVec {
    pub fn new(inner: Arc<Vec<SnapshotValue>>) -> Arc<Self> {
        Arc::new(Self { inner })
    }

    pub fn materialize(&self, env: &GlobalEnv) -> Result<Value, VmError> {
        SnapshotValue::Vec(self.inner.clone()).rebuild(env)
    }

    fn get(&self, key: &Value) -> Option<&SnapshotValue> {
        let i = match key {
            Value::Integer(n) => *n,
            _ => return None,
        };
        // Lua's 1-based indexing; outside the range returns nil.
        if i < 1 {
            return None;
        }
        let idx = (i - 1) as usize;
        self.inner.get(idx)
    }
}

#[async_trait]
impl Userdata for LuaSnapshotVec {
    fn type_name(&self) -> &'static str {
        "snapshot_vec"
    }

    fn lua_type_info(&self) -> LuaType {
        LuaType::named("snapshot_vec")
    }

    fn has_metamethod(&self, name: &str) -> bool {
        matches!(
            name,
            "__index" | "__newindex" | "__len" | "__tostring" | "__pairs" | "__ipairs"
        )
    }

    fn pretty_entries<'a>(&'a self, env: &GlobalEnv) -> Option<Result<PrettyShape<'a>, VmError>> {
        let env = env.clone();
        let iter = self.inner.iter().map(move |v| v.rebuild_lazy(&env));
        Some(Ok(PrettyShape::Vec(Box::new(iter))))
    }

    fn index(&self, key: &Value) -> Option<Result<ValueVec, VmError>> {
        match self.get(key) {
            None => Some(Ok(valuevec![Value::Nil])),
            Some(SnapshotValue::Map(_))
            | Some(SnapshotValue::Vec(_))
            | Some(SnapshotValue::Snapshot(_)) => None,
            Some(v) => Some(v.rebuild_no_env().map(|val| valuevec![val])),
        }
    }

    fn newindex(&self, _key: &Value, _value: &Value) -> Option<Result<ValueVec, VmError>> {
        Some(Err(frozen_write_error()))
    }

    async fn dispatch(
        self: Arc<Self>,
        ctx: CallContext,
        metamethod: &str,
        args: ValueVec,
    ) -> Result<ValueVec, VmError> {
        match metamethod {
            "__index" => {
                let key = args.get(1).cloned().unwrap_or(Value::Nil);
                match self.get(&key) {
                    None => Ok(valuevec![Value::Nil]),
                    Some(sv) => Ok(valuevec![sv.rebuild_lazy(&ctx.global)?]),
                }
            }
            "__newindex" => Err(frozen_write_error()),
            "__len" => Ok(valuevec![Value::Integer(self.inner.len() as i64)]),
            "__tostring" => Ok(valuevec![Value::string(format!(
                "<snapshot_vec: {} entries>",
                self.inner.len()
            ))]),
            // `__pairs` and `__ipairs` are equivalent on a vec: both
            // iterate `1..=len` in order.
            "__pairs" | "__ipairs" => Ok(self.build_iter_triple(ctx.global.clone())),
            other => Err(VmError::HostError {
                name: "snapshot_vec".to_owned(),
                source: format!("metamethod {other} not implemented on snapshot_vec").into(),
            }),
        }
    }
}

impl LuaSnapshotVec {
    /// Build the `(iter_fn, state, control)` triple shared by
    /// `__pairs` and `__ipairs`.  Walks `1..=len` in order, lazily
    /// rebuilding each value in the captured env.
    fn build_iter_triple(&self, env: GlobalEnv) -> ValueVec {
        let inner = self.inner.clone();
        let pos = Arc::new(AtomicUsize::new(0));
        let iter_fn = Function::wrap(
            "snapshot_vec_iter",
            move || -> Result<(Value, Value), VmError> {
                let i = pos.fetch_add(1, Ordering::SeqCst);
                match inner.get(i) {
                    None => Ok((Value::Nil, Value::Nil)),
                    Some(v) => Ok((Value::Integer((i + 1) as i64), v.rebuild_lazy(&env)?)),
                }
            },
        );
        valuevec![Value::Function(iter_fn), Value::Nil, Value::Nil]
    }
}
