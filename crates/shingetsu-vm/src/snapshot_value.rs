//! Cross-VM snapshot of a Lua [`Value`].
//!
//! [`SnapshotValue`] captures a Lua value into a form that's safe to
//! share across [`GlobalEnv`] instances and async boundaries.  The
//! capture path ([`FromLua::from_lua`]) deep-copies tables, captures
//! primitives + strings directly, and dispatches to a userdata's
//! [`Snapshot`] handler when present.  The rebuild path
//! ([`SnapshotValue::rebuild`]) re-materializes a fresh Lua value in
//! any env.
//!
//! Used by host-shared sync primitives (`task.watch`, `task.channel`,
//! etc.) that need to transport values across VM boundaries without
//! aliasing the producer's tables or capturing the producer's
//! upvalues.
//!
//! # Type coverage
//!
//! | Lua type                      | Variant     | Notes |
//! | ----------------------------- | ----------- | ----- |
//! | nil                           | `Nil`       | |
//! | boolean                       | `Boolean`   | |
//! | integer                       | `Integer`   | |
//! | float                         | `Float`     | |
//! | string                        | `String`    | `Bytes` clone is O(1) |
//! | array-shape table             | `Vec`       | consecutive integer keys `1..=n`, `n > 0` |
//! | other table                   | `Map`       | int/string keys; cycles rejected |
//! | userdata with `snapshot()`    | `Snapshot`  | rebuilt via the per-type closure |
//! | function                      | rejected    | upvalues bound to a specific env |
//! | userdata without `snapshot()` | rejected    | type opted out of cross-VM transport |
//!
//! Cyclic tables are rejected at capture time.
//!
//! # Eager vs lazy rebuild
//!
//! [`SnapshotValue::rebuild`] walks the whole tree and allocates a
//! fresh Lua [`Table`] for every level.
//! [`SnapshotValue::rebuild_lazy`] returns a userdata proxy for
//! `Map` and `Vec` variants; the proxy lazily rebuilds only the
//! values that are accessed.  See `crate::snapshot_table` for the
//! proxy types.

use std::collections::HashSet;
use std::sync::Arc;

use indexmap::IndexMap;

use crate::byte_string::Bytes;
use crate::convert::{FromLua, LuaTyped};
use crate::error::VmError;
use crate::global_env::GlobalEnv;
use crate::snapshot_table::{LuaSnapshotMap, LuaSnapshotVec};
use crate::table::{Table, TableShape};
use crate::types::LuaType;
use crate::userdata::Snapshot;
use crate::value::Value;

/// A captured Lua value, safe to share across VM instances.
#[derive(Clone)]
pub enum SnapshotValue {
    Nil,
    Boolean(bool),
    Integer(i64),
    Float(f64),
    String(Bytes),
    /// Recursively snapshotted associative table.  Captured when
    /// the source table has any non-array shape (string keys,
    /// sparse integer keys, integer keys not starting at 1, or
    /// empty).  `Arc` lets multiple consumers share the same
    /// captured tree without re-walking it.
    Map(Arc<IndexMap<MapKey, SnapshotValue>>),
    /// Recursively snapshotted array-shape table.  Captured when
    /// the source table has exactly the integer keys `1..=n` for
    /// some `n > 0` and no other keys.
    Vec(Arc<std::vec::Vec<SnapshotValue>>),
    /// Userdata that opted into snapshotting via [`Snapshot`].
    Snapshot(Snapshot),
}

/// Permitted key types for snapshotted tables.  Lua tables can use
/// any value as a key, but only integer and string keys can be safely
/// transported across VMs (other types either alias mutable state,
/// have non-deterministic identity, or have no rebuild path).
#[derive(Clone, Hash, Eq, PartialEq, Debug)]
pub enum MapKey {
    Integer(i64),
    String(Bytes),
}

impl LuaTyped for SnapshotValue {
    fn lua_type() -> LuaType {
        // SnapshotValue accepts any Lua value at the type-system
        // level; non-snapshottable inputs are rejected at runtime
        // by `from_lua` with a clear diagnostic.
        LuaType::Any
    }
}

impl FromLua for SnapshotValue {
    fn from_lua(v: Value, _env: &GlobalEnv) -> Result<Self, VmError> {
        let mut visited = HashSet::new();
        Self::from_lua_inner(&v, &mut visited)
    }

    fn from_lua_ref(v: &Value, _env: &GlobalEnv) -> Result<Self, VmError> {
        let mut visited = HashSet::new();
        Self::from_lua_inner(v, &mut visited)
    }
}

impl SnapshotValue {
    fn from_lua_inner(v: &Value, visited: &mut HashSet<usize>) -> Result<Self, VmError> {
        match v {
            Value::Nil => Ok(Self::Nil),
            Value::Boolean(b) => Ok(Self::Boolean(*b)),
            Value::Integer(n) => Ok(Self::Integer(*n)),
            Value::Float(f) => Ok(Self::Float(*f)),
            Value::String(s) => Ok(Self::String(s.clone())),
            Value::Table(t) => Self::from_table(t, visited),
            Value::Function(_) => Err(snapshot_error(
                "function values cannot be snapshotted (functions \
                 capture upvalues bound to a specific environment)",
            )),
            Value::Userdata(ud) => match ud.snapshot() {
                Some(s) => Ok(Self::Snapshot(s)),
                None => Err(snapshot_error(format!(
                    "userdata of type {:?} cannot be snapshotted (type \
                     does not opt in to cross-environment transport)",
                    ud.type_name(),
                ))),
            },
        }
    }

    fn from_table(t: &Table, visited: &mut HashSet<usize>) -> Result<Self, VmError> {
        let id = t.identity();
        if !visited.insert(id) {
            return Err(snapshot_error("cyclic table cannot be snapshotted"));
        }

        let snap = match t.detect_shape()? {
            TableShape::Vec { len } => {
                let mut out = std::vec::Vec::with_capacity(len);
                for i in 1..=len {
                    let v = t.raw_get(&Value::Integer(i as i64))?;
                    out.push(Self::from_lua_inner(&v, visited)?);
                }
                Self::Vec(Arc::new(out))
            }
            TableShape::Map => {
                let mut map = IndexMap::new();
                let mut k = Value::Nil;
                while let Some((nk, nv)) = t.next(&k)? {
                    let key = match &nk {
                        Value::Integer(n) => MapKey::Integer(*n),
                        Value::String(s) => MapKey::String(s.clone()),
                        other => {
                            return Err(snapshot_error(format!(
                                "table key of type {} cannot be snapshotted; \
                                 only integer and string keys are supported",
                                other.type_name(),
                            )));
                        }
                    };
                    let val = Self::from_lua_inner(&nv, visited)?;
                    map.insert(key, val);
                    k = nk;
                }
                Self::Map(Arc::new(map))
            }
        };
        visited.remove(&id);
        Ok(snap)
    }

    /// Re-materialize this snapshot as a fresh Lua [`Value`] in `env`.
    ///
    /// Allocates new [`Table`]s for every `Map` and `Vec` variant so
    /// the resulting values can be mutated freely without affecting
    /// the snapshot or other consumers' rebuilt copies.
    pub fn rebuild(&self, env: &GlobalEnv) -> Result<Value, VmError> {
        match self {
            Self::Nil => Ok(Value::Nil),
            Self::Boolean(b) => Ok(Value::Boolean(*b)),
            Self::Integer(n) => Ok(Value::Integer(*n)),
            Self::Float(f) => Ok(Value::Float(*f)),
            Self::String(s) => Ok(Value::String(s.clone())),
            Self::Snapshot(s) => s.rebuild(env),
            Self::Map(map) => {
                let table = Table::new();
                env.track_table(&table);
                for (k, v) in map.iter() {
                    let key = match k {
                        MapKey::Integer(n) => Value::Integer(*n),
                        MapKey::String(s) => Value::String(s.clone()),
                    };
                    let val = v.rebuild(env)?;
                    table.raw_set(key, val)?;
                }
                Ok(Value::Table(table))
            }
            Self::Vec(items) => {
                let table = Table::new();
                env.track_table(&table);
                for (i, v) in items.iter().enumerate() {
                    table.raw_set(Value::Integer((i + 1) as i64), v.rebuild(env)?)?;
                }
                Ok(Value::Table(table))
            }
        }
    }

    /// Lazy variant of [`Self::rebuild`].  Container variants (`Map`,
    /// `Vec`) return a read-only userdata proxy that lazily rebuilds
    /// values as they are accessed.  Non-container variants take the
    /// same path as eager `rebuild`.
    ///
    /// Used by host-shared primitives that may return large captured
    /// values to consumers who only read a narrow subset (config
    /// lookups in `task.watch`, message inspection in
    /// `task.bounded_channel`, etc.).
    pub fn rebuild_lazy(&self, env: &GlobalEnv) -> Result<Value, VmError> {
        match self {
            Self::Map(map) => Ok(Value::Userdata(LuaSnapshotMap::new(map.clone()))),
            Self::Vec(items) => Ok(Value::Userdata(LuaSnapshotVec::new(items.clone()))),
            other => other.rebuild(env),
        }
    }

    /// Rebuild without needing a [`GlobalEnv`].  Only the non-table,
    /// non-snapshot variants succeed; the others return an error.
    ///
    /// Used by the snapshot-table proxies' sync `__index` fast path
    /// for leaf values (primitives, strings) where no env is
    /// available.  Container and userdata-snapshot variants fall
    /// through to the async dispatch path which has a `CallContext`.
    pub(crate) fn rebuild_no_env(&self) -> Result<Value, VmError> {
        match self {
            Self::Nil => Ok(Value::Nil),
            Self::Boolean(b) => Ok(Value::Boolean(*b)),
            Self::Integer(n) => Ok(Value::Integer(*n)),
            Self::Float(f) => Ok(Value::Float(*f)),
            Self::String(s) => Ok(Value::String(s.clone())),
            Self::Map(_) | Self::Vec(_) | Self::Snapshot(_) => Err(VmError::HostError {
                name: "snapshot".to_owned(),
                source: "rebuild_no_env called on container/snapshot variant".into(),
            }),
        }
    }
}

impl std::fmt::Debug for SnapshotValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Nil => write!(f, "Nil"),
            Self::Boolean(b) => f.debug_tuple("Boolean").field(b).finish(),
            Self::Integer(n) => f.debug_tuple("Integer").field(n).finish(),
            Self::Float(x) => f.debug_tuple("Float").field(x).finish(),
            Self::String(s) => f.debug_tuple("String").field(&bstr::BStr::new(s)).finish(),
            Self::Snapshot(_) => f.debug_struct("Snapshot").finish_non_exhaustive(),
            Self::Map(t) => f.debug_tuple("Map").field(&t.len()).finish(),
            Self::Vec(t) => f.debug_tuple("Vec").field(&t.len()).finish(),
        }
    }
}

fn snapshot_error(msg: impl Into<String>) -> VmError {
    let msg = msg.into();
    VmError::HostError {
        name: "snapshot".to_owned(),
        source: msg.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::userdata::Userdata;

    fn debug_eq(left: &Value, right: &Value) {
        // Value doesn't impl PartialEq for all variants by content;
        // compare via Debug output, which is sufficient for the
        // primitive + table cases exercised here.
        k9::assert_equal!(format!("{left:?}"), format!("{right:?}"));
    }

    #[test]
    fn roundtrip_primitives() {
        let env = GlobalEnv::new();
        for v in [
            Value::Nil,
            Value::Boolean(true),
            Value::Boolean(false),
            Value::Integer(42),
            Value::Integer(-7),
            Value::Float(3.5),
            Value::String("hello".into()),
        ] {
            let snap = SnapshotValue::from_lua(v.clone(), &env).expect("capture");
            let back = snap.rebuild(&env).expect("rebuild");
            debug_eq(&back, &v);
        }
    }

    #[test]
    fn roundtrip_table_with_mixed_keys() {
        let env = GlobalEnv::new();
        let t = Table::new();
        t.raw_set(Value::Integer(1), Value::string("first"))
            .unwrap();
        t.raw_set(Value::Integer(2), Value::Integer(99)).unwrap();
        t.raw_set(Value::string("name"), Value::string("widget"))
            .unwrap();
        let snap =
            SnapshotValue::from_lua(Value::Table(t), &crate::GlobalEnv::new()).expect("capture");
        let rebuilt = match snap.rebuild(&env).expect("rebuild") {
            Value::Table(t) => t,
            other => panic!("expected Table, got {other:?}"),
        };
        debug_eq(
            &rebuilt.raw_get(&Value::Integer(1)).unwrap(),
            &Value::string("first"),
        );
        debug_eq(
            &rebuilt.raw_get(&Value::Integer(2)).unwrap(),
            &Value::Integer(99),
        );
        debug_eq(
            &rebuilt.raw_get(&Value::string("name")).unwrap(),
            &Value::string("widget"),
        );
    }

    #[test]
    fn roundtrip_nested_table() {
        let env = GlobalEnv::new();
        let inner = Table::new();
        inner
            .raw_set(Value::string("k"), Value::Integer(1))
            .unwrap();
        let outer = Table::new();
        outer
            .raw_set(Value::string("inner"), Value::Table(inner))
            .unwrap();
        let snap = SnapshotValue::from_lua(Value::Table(outer), &crate::GlobalEnv::new())
            .expect("capture");
        let rebuilt = match snap.rebuild(&env).expect("rebuild") {
            Value::Table(t) => t,
            other => panic!("expected Table, got {other:?}"),
        };
        let nested = match rebuilt.raw_get(&Value::string("inner")).unwrap() {
            Value::Table(t) => t,
            other => panic!("expected nested Table, got {other:?}"),
        };
        debug_eq(
            &nested.raw_get(&Value::string("k")).unwrap(),
            &Value::Integer(1),
        );
    }

    #[test]
    fn rebuilt_table_is_independent_of_snapshot() {
        // Mutating a rebuilt table must not affect the snapshot or
        // tables rebuilt elsewhere.
        let env = GlobalEnv::new();
        let t = Table::new();
        t.raw_set(Value::string("k"), Value::Integer(1)).unwrap();
        let snap =
            SnapshotValue::from_lua(Value::Table(t), &crate::GlobalEnv::new()).expect("capture");
        let a = match snap.rebuild(&env).unwrap() {
            Value::Table(t) => t,
            _ => unreachable!(),
        };
        let b = match snap.rebuild(&env).unwrap() {
            Value::Table(t) => t,
            _ => unreachable!(),
        };
        a.raw_set(Value::string("k"), Value::Integer(99)).unwrap();
        debug_eq(
            &a.raw_get(&Value::string("k")).unwrap(),
            &Value::Integer(99),
        );
        debug_eq(&b.raw_get(&Value::string("k")).unwrap(), &Value::Integer(1));
    }

    #[test]
    fn shape_detection_contiguous_int_keys_is_vec() {
        let t = Table::new();
        t.raw_set(Value::Integer(1), Value::string("a")).unwrap();
        t.raw_set(Value::Integer(2), Value::string("b")).unwrap();
        t.raw_set(Value::Integer(3), Value::string("c")).unwrap();
        let snap =
            SnapshotValue::from_lua(Value::Table(t), &crate::GlobalEnv::new()).expect("capture");
        match snap {
            SnapshotValue::Vec(ref items) => {
                k9::assert_equal!(items.len(), 3);
            }
            other => panic!("expected Vec, got {other:?}"),
        }
    }

    #[test]
    fn shape_detection_sparse_int_keys_is_map() {
        let t = Table::new();
        t.raw_set(Value::Integer(1), Value::string("a")).unwrap();
        t.raw_set(Value::Integer(3), Value::string("c")).unwrap();
        let snap =
            SnapshotValue::from_lua(Value::Table(t), &crate::GlobalEnv::new()).expect("capture");
        match snap {
            SnapshotValue::Map(_) => {}
            other => panic!("expected Map, got {other:?}"),
        }
    }

    #[test]
    fn shape_detection_zero_indexed_is_map() {
        // Int keys starting at 0 are not the Lua array convention.
        let t = Table::new();
        t.raw_set(Value::Integer(0), Value::string("a")).unwrap();
        t.raw_set(Value::Integer(1), Value::string("b")).unwrap();
        let snap =
            SnapshotValue::from_lua(Value::Table(t), &crate::GlobalEnv::new()).expect("capture");
        match snap {
            SnapshotValue::Map(_) => {}
            other => panic!("expected Map, got {other:?}"),
        }
    }

    #[test]
    fn shape_detection_mixed_keys_is_map() {
        let t = Table::new();
        t.raw_set(Value::Integer(1), Value::string("a")).unwrap();
        t.raw_set(Value::string("name"), Value::string("x"))
            .unwrap();
        let snap =
            SnapshotValue::from_lua(Value::Table(t), &crate::GlobalEnv::new()).expect("capture");
        match snap {
            SnapshotValue::Map(_) => {}
            other => panic!("expected Map, got {other:?}"),
        }
    }

    #[test]
    fn shape_detection_empty_is_map() {
        let snap = SnapshotValue::from_lua(Value::Table(Table::new()), &crate::GlobalEnv::new())
            .expect("capture");
        match snap {
            SnapshotValue::Map(ref m) => {
                k9::assert_equal!(m.len(), 0);
            }
            other => panic!("expected Map, got {other:?}"),
        }
    }

    #[test]
    fn vec_rebuild_preserves_order() {
        let env = GlobalEnv::new();
        let t = Table::new();
        for i in 1..=5 {
            t.raw_set(Value::Integer(i), Value::Integer(i * 10))
                .unwrap();
        }
        let snap =
            SnapshotValue::from_lua(Value::Table(t), &crate::GlobalEnv::new()).expect("capture");
        let rebuilt = match snap.rebuild(&env).expect("rebuild") {
            Value::Table(t) => t,
            other => panic!("expected Table, got {other:?}"),
        };
        k9::assert_equal!(rebuilt.raw_len(), 5);
        for i in 1..=5 {
            debug_eq(
                &rebuilt.raw_get(&Value::Integer(i)).unwrap(),
                &Value::Integer(i * 10),
            );
        }
    }

    #[test]
    fn cyclic_table_rejected() {
        let t = Table::new();
        t.raw_set(Value::string("self"), Value::Table(t.clone()))
            .unwrap();
        let err =
            SnapshotValue::from_lua(Value::Table(t), &crate::GlobalEnv::new()).expect_err("cycle");
        k9::assert_equal!(
            err.to_string(),
            "error in 'snapshot': cyclic table cannot be snapshotted"
        );
    }

    #[test]
    fn function_rejected() {
        let f = crate::function::Function::wrap("noop", || Ok::<(), VmError>(()));
        let err = SnapshotValue::from_lua(Value::Function(f), &crate::GlobalEnv::new())
            .expect_err("function");
        k9::assert_equal!(
            err.to_string(),
            "error in 'snapshot': function values cannot be snapshotted \
             (functions capture upvalues bound to a specific environment)"
        );
    }

    #[test]
    fn opted_out_userdata_rejected() {
        struct Plain;
        impl Userdata for Plain {
            fn type_name(&self) -> &'static str {
                "Plain"
            }
        }
        let v = Value::userdata(Arc::new(Plain));
        let env = crate::GlobalEnv::new();
        let err = SnapshotValue::from_lua(v, &env).expect_err("userdata");
        k9::assert_equal!(
            err.to_string(),
            "error in 'snapshot': userdata of type \"Plain\" cannot be \
             snapshotted (type does not opt in to cross-environment transport)"
        );
    }

    #[test]
    fn opted_in_userdata_round_trips() {
        struct Counted(i64);
        impl Userdata for Counted {
            fn type_name(&self) -> &'static str {
                "Counted"
            }
            fn snapshot(&self) -> Option<Snapshot> {
                let n = self.0;
                Some(Snapshot::new(move |_env| Ok(Value::Integer(n))))
            }
        }
        let env = GlobalEnv::new();
        let v = Value::userdata(Arc::new(Counted(7)));
        let snap = SnapshotValue::from_lua(v, &env).expect("capture");
        let back = snap.rebuild(&env).expect("rebuild");
        debug_eq(&back, &Value::Integer(7));
    }

    #[test]
    fn table_with_function_value_rejected() {
        let f = crate::function::Function::wrap("noop", || Ok::<(), VmError>(()));
        let t = Table::new();
        t.raw_set(Value::string("f"), Value::Function(f)).unwrap();
        let err = SnapshotValue::from_lua(Value::Table(t), &crate::GlobalEnv::new())
            .expect_err("function in table");
        // Same diagnostic as the bare function case; recursion bubbles up.
        k9::assert_equal!(
            err.to_string(),
            "error in 'snapshot': function values cannot be snapshotted \
             (functions capture upvalues bound to a specific environment)"
        );
    }

    #[test]
    fn table_with_bad_key_type_rejected() {
        let t = Table::new();
        t.raw_set(Value::Boolean(true), Value::Integer(1)).unwrap();
        let err = SnapshotValue::from_lua(Value::Table(t), &crate::GlobalEnv::new())
            .expect_err("bad key");
        k9::assert_equal!(
            err.to_string(),
            "error in 'snapshot': table key of type boolean cannot be \
             snapshotted; only integer and string keys are supported"
        );
    }
}
