//! Cross-engine `wezterm-dynamic` interop.
//!
//! [`DynamicLua<T>`] is a newtype wrapper around any
//! `T: FromDynamic + ToDynamic` (the wezterm-dynamic conversion
//! trait pair) that implements both engines' lua-conversion traits.
//! It is the migration target for wezterm's
//! `impl_lua_conversion_dynamic!(T)` macro: hosts replace the macro
//! invocation with a `DynamicLua<T>` wrapping at the relevant call
//! sites and the type round-trips through either engine via the
//! shared `wezterm_dynamic::Value` tree.
//!
//! The mlua-side conversion is structurally the same as wezterm's
//! `luahelper::dynamic_to_lua_value` / `lua_value_to_dynamic`,
//! inlined here because `luahelper` is wezterm-internal and pulls
//! in non-portable dependencies.  The shingetsu side is the
//! analogous walk over `shingetsu::Value`.
//!
//! Cycles in the input value are mapped to `Null` on the way to
//! `wezterm_dynamic::Value` (matching luahelper's behavior).

#![cfg(feature = "dynamic")]

use std::collections::HashSet;

use wezterm_dynamic::{FromDynamic, ToDynamic, Value as DynValue};

/// Cross-engine adapter: wraps a `T` whose canonical "dynamic"
/// representation is `wezterm_dynamic::Value`, providing FromLua /
/// IntoLua impls on whichever engines are enabled.
#[derive(Debug, Clone, PartialEq)]
pub struct DynamicLua<T>(pub T);

impl<T> DynamicLua<T> {
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> From<T> for DynamicLua<T> {
    fn from(v: T) -> Self {
        Self(v)
    }
}

// ---------------------------------------------------------------------------
// shingetsu-backend impls
// ---------------------------------------------------------------------------

#[cfg(feature = "shingetsu-backend")]
mod shingetsu_impls {
    use super::*;
    use shingetsu::{Bytes, FromLua, IntoLua, Table, Value, VmError};

    impl<T: FromDynamic> FromLua for DynamicLua<T> {
        fn from_lua(v: Value) -> Result<Self, VmError> {
            let dyn_value = shingetsu_value_to_dynamic(v)?;
            T::from_dynamic(&dyn_value, Default::default())
                .map(DynamicLua)
                .map_err(|e| VmError::LuaError {
                    display: format!("DynamicLua::from_lua: {e}"),
                    value: Value::string(format!("DynamicLua::from_lua: {e}")),
                })
        }
    }

    impl<T: ToDynamic> IntoLua for DynamicLua<T> {
        fn into_lua(self) -> Value {
            dynamic_to_shingetsu_value(self.0.to_dynamic())
        }
    }

    pub(super) fn dynamic_to_shingetsu_value(v: DynValue) -> Value {
        match v {
            DynValue::Null => Value::Nil,
            DynValue::Bool(b) => Value::Boolean(b),
            DynValue::String(s) => Value::String(Bytes::from(s.into_bytes())),
            DynValue::I64(i) => Value::Integer(i),
            DynValue::U64(u) => {
                if u <= i64::MAX as u64 {
                    Value::Integer(u as i64)
                } else {
                    Value::Float(u as f64)
                }
            }
            DynValue::F64(f) => Value::Float(*f),
            DynValue::Array(arr) => {
                let table = Table::new();
                for (idx, item) in arr.into_iter().enumerate() {
                    let _ = table.raw_set(
                        Value::Integer((idx + 1) as i64),
                        dynamic_to_shingetsu_value(item),
                    );
                }
                Value::Table(table)
            }
            DynValue::Object(obj) => {
                let table = Table::new();
                for (k, v) in obj.into_iter() {
                    let _ =
                        table.raw_set(dynamic_to_shingetsu_value(k), dynamic_to_shingetsu_value(v));
                }
                Value::Table(table)
            }
        }
    }

    pub(super) fn shingetsu_value_to_dynamic(v: Value) -> Result<DynValue, VmError> {
        let mut visited = HashSet::new();
        shingetsu_value_to_dynamic_impl(v, &mut visited)
    }

    fn cannot_map(what: &str) -> VmError {
        let display = format!("DynamicLua: {what} cannot convert to wezterm_dynamic::Value");
        let value = Value::string(display.clone());
        VmError::LuaError { display, value }
    }

    fn shingetsu_value_to_dynamic_impl(
        v: Value,
        visited: &mut HashSet<usize>,
    ) -> Result<DynValue, VmError> {
        Ok(match v {
            Value::Nil => DynValue::Null,
            Value::Boolean(b) => DynValue::Bool(b),
            Value::Integer(i) => DynValue::I64(i),
            Value::Float(f) => DynValue::F64(f.into()),
            Value::String(s) => match std::str::from_utf8(s.as_ref()) {
                Ok(text) => DynValue::String(text.to_owned()),
                Err(_) => return Err(cannot_map("non-utf8 string")),
            },
            Value::Table(t) => {
                // Cycle break by table-pointer identity, matching the
                // luahelper convention that maps revisited nodes to
                // `Null`.
                let ptr = t.identity();
                if !visited.insert(ptr) {
                    return Ok(DynValue::Null);
                }
                table_to_dynamic(&t, visited)?
            }
            Value::Function(_) => return Err(cannot_map("function values")),
            Value::Userdata(_) => return Err(cannot_map("userdata values")),
        })
    }

    fn table_to_dynamic(t: &Table, visited: &mut HashSet<usize>) -> Result<DynValue, VmError> {
        // Decide array-vs-object by walking once: if all keys are
        // dense integer keys starting at 1, treat as Array; else
        // Object.  Mirrors how Lua-flavored serializers usually
        // shape this distinction.
        let mut entries: Vec<(Value, Value)> = Vec::new();
        let mut key = Value::Nil;
        loop {
            match t.next(&key)? {
                Some((k, v)) => {
                    entries.push((k.clone(), v));
                    key = k;
                }
                None => break,
            }
        }
        let is_array = !entries.is_empty()
            && entries
                .iter()
                .enumerate()
                .all(|(idx, (k, _))| matches!(k, Value::Integer(i) if *i == (idx as i64 + 1)));
        if is_array {
            let mut arr = wezterm_dynamic::Array::default();
            for (_, v) in entries {
                arr.push(shingetsu_value_to_dynamic_impl(v, visited)?);
            }
            Ok(DynValue::Array(arr))
        } else {
            let mut obj = wezterm_dynamic::Object::default();
            for (k, v) in entries {
                let dk = shingetsu_value_to_dynamic_impl(k, visited)?;
                let dv = shingetsu_value_to_dynamic_impl(v, visited)?;
                obj.insert(dk, dv);
            }
            Ok(DynValue::Object(obj))
        }
    }
}

// ---------------------------------------------------------------------------
// mlua-backend impls
// ---------------------------------------------------------------------------

#[cfg(feature = "mlua-backend")]
mod mlua_impls {
    use super::*;

    impl<T: FromDynamic> mlua::FromLua for DynamicLua<T> {
        fn from_lua(value: mlua::Value, _lua: &mlua::Lua) -> mlua::Result<Self> {
            let lua_type = value.type_name();
            let dyn_value =
                lua_value_to_dynamic(value).map_err(|e| mlua::Error::FromLuaConversionError {
                    from: lua_type,
                    to: std::any::type_name::<T>().to_owned(),
                    message: Some(e.to_string()),
                })?;
            T::from_dynamic(&dyn_value, Default::default())
                .map(DynamicLua)
                .map_err(|e| mlua::Error::FromLuaConversionError {
                    from: lua_type,
                    to: std::any::type_name::<T>().to_owned(),
                    message: Some(e.to_string()),
                })
        }
    }

    impl<T: ToDynamic> mlua::IntoLua for DynamicLua<T> {
        fn into_lua(self, lua: &mlua::Lua) -> mlua::Result<mlua::Value> {
            dynamic_to_lua_value(lua, self.0.to_dynamic())
        }
    }

    fn dynamic_to_lua_value(lua: &mlua::Lua, value: DynValue) -> mlua::Result<mlua::Value> {
        Ok(match value {
            DynValue::Null => mlua::Value::Nil,
            DynValue::Bool(b) => mlua::Value::Boolean(b),
            DynValue::String(s) => mlua::IntoLua::into_lua(s, lua)?,
            DynValue::U64(u) => mlua::IntoLua::into_lua(u, lua)?,
            DynValue::I64(i) => mlua::IntoLua::into_lua(i, lua)?,
            DynValue::F64(f) => mlua::IntoLua::into_lua(*f, lua)?,
            DynValue::Array(arr) => {
                let table = lua.create_table()?;
                for (idx, item) in arr.into_iter().enumerate() {
                    table.set(idx + 1, dynamic_to_lua_value(lua, item)?)?;
                }
                mlua::Value::Table(table)
            }
            DynValue::Object(obj) => {
                let table = lua.create_table()?;
                for (k, v) in obj.into_iter() {
                    table.set(dynamic_to_lua_value(lua, k)?, dynamic_to_lua_value(lua, v)?)?;
                }
                mlua::Value::Table(table)
            }
        })
    }

    fn lua_value_to_dynamic(value: mlua::Value) -> mlua::Result<DynValue> {
        let mut visited = HashSet::new();
        lua_value_to_dynamic_impl(value, &mut visited)
    }

    fn lua_value_to_dynamic_impl(
        value: mlua::Value,
        visited: &mut HashSet<usize>,
    ) -> mlua::Result<DynValue> {
        if matches!(value, mlua::Value::Table(_)) {
            let ptr = value.to_pointer() as usize;
            if !visited.insert(ptr) {
                return Ok(DynValue::Null);
            }
        }
        Ok(match value {
            mlua::Value::Nil => DynValue::Null,
            mlua::Value::Boolean(b) => DynValue::Bool(b),
            mlua::Value::Integer(i) => DynValue::I64(i),
            mlua::Value::Number(f) => DynValue::F64(f.into()),
            mlua::Value::String(s) => DynValue::String(s.to_str()?.to_owned()),
            mlua::Value::Table(t) => {
                // Decide array-vs-object by walking once.
                let mut entries: Vec<(mlua::Value, mlua::Value)> = Vec::new();
                for pair in t.pairs::<mlua::Value, mlua::Value>() {
                    let (k, v) = pair?;
                    entries.push((k, v));
                }
                let is_array = !entries.is_empty()
                    && entries.iter().enumerate().all(|(idx, (k, _))| {
                        matches!(k, mlua::Value::Integer(i) if *i == (idx as i64 + 1))
                    });
                if is_array {
                    let mut arr = wezterm_dynamic::Array::default();
                    for (_, v) in entries {
                        arr.push(lua_value_to_dynamic_impl(v, visited)?);
                    }
                    DynValue::Array(arr)
                } else {
                    let mut obj = wezterm_dynamic::Object::default();
                    for (k, v) in entries {
                        let dk = lua_value_to_dynamic_impl(k, visited)?;
                        let dv = lua_value_to_dynamic_impl(v, visited)?;
                        obj.insert(dk, dv);
                    }
                    DynValue::Object(obj)
                }
            }
            mlua::Value::LightUserData(_)
            | mlua::Value::Function(_)
            | mlua::Value::Thread(_)
            | mlua::Value::UserData(_) => {
                return Err(mlua::Error::external(format!(
                    "DynamicLua: {} values cannot convert to wezterm_dynamic::Value",
                    value.type_name()
                )));
            }
            mlua::Value::Error(e) => return Err(*e),
            #[allow(unreachable_patterns)]
            _ => DynValue::Null,
        })
    }
}
