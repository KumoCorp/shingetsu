//! Serde-backed `__pairs` / `__index` / `__len` for opaque
//! `Serialize` userdata.
//!
//! Mirrors kumomta's `config::impl_pairs_and_index`: a userdata
//! whose only lua-visible behavior is "read me like a table" (the
//! values come from the type's `serde` representation).  Enabled
//! via `#[shingetsu_migrate::userdata(serde_index)]`.
//!
//! - mlua backend: [`install_mlua`] registers the exact
//!   `Pairs` / `Index` / `Len` metamethods kumomta historically
//!   used, so behavior on the running engine is byte-for-byte
//!   preserved.
//! - shingetsu backend: the macro wires the generated `Userdata`
//!   impl's `index` / `__len` / `__pairs` to the [`shingetsu`]
//!   helpers here, driven by the same `serde_json` representation.

// ---------------------------------------------------------------------------
// mlua backend — exact parity with `config::impl_pairs_and_index`
// ---------------------------------------------------------------------------

#[cfg(feature = "mlua-backend")]
pub fn install_mlua<T, M>(methods: &mut M)
where
    T: mlua::UserData + serde::Serialize,
    M: mlua::UserDataMethods<T>,
{
    use mlua::{LuaSerdeExt, MetaMethod, Value};

    methods.add_meta_method(MetaMethod::Pairs, move |lua, this, _: ()| {
        let Ok(serde_json::Value::Object(map)) =
            serde_json::to_value(this).map_err(mlua::Error::external)
        else {
            return Err(mlua::Error::external("must serialize to Map"));
        };

        let mut value_iter = map.into_iter();

        let iter_func = lua.create_function_mut(
            move |lua, (_state, _control): (Value, Value)| match value_iter.next() {
                Some((key, value)) => {
                    let key = lua.to_value(&key)?;
                    let value = lua.to_value(&value)?;
                    Ok((key, value))
                }
                None => Ok((Value::Nil, Value::Nil)),
            },
        )?;

        Ok((Value::Function(iter_func), Value::Nil, Value::Nil))
    });

    methods.add_meta_method(MetaMethod::Index, move |lua, this, field: Value| {
        let value = lua.to_value(this)?;
        match value {
            Value::Table(t) => t.get(field),
            _ => Ok(Value::Nil),
        }
    });

    methods.add_meta_method(MetaMethod::Len, move |lua, this, _: ()| {
        let value = lua.to_value(this)?;
        match value {
            Value::Table(v) => v.len(),
            Value::String(v) => Ok(v.as_bytes().len() as i64),
            _ => Ok(0),
        }
    });
}

// ---------------------------------------------------------------------------
// shingetsu backend
// ---------------------------------------------------------------------------

#[cfg(feature = "shingetsu-backend")]
pub mod shingetsu {
    use ::shingetsu::serde_bridge::to_value;
    use ::shingetsu::{Function, Value, ValueVec, VmError};

    /// Serialize `this` straight to a [`Value`] (no `serde_json`
    /// intermediary) and treat the result as a table.
    fn as_table<T: serde::Serialize>(this: &T) -> Result<Option<::shingetsu::Table>, VmError> {
        match to_value(this)? {
            Value::Table(t) => Ok(Some(t)),
            _ => Ok(None),
        }
    }

    /// `Userdata::index` fast-path body: field read from the serde
    /// representation.
    pub fn index<T: serde::Serialize>(this: &T, key: &Value) -> Option<Result<ValueVec, VmError>> {
        let table = match as_table(this) {
            Ok(Some(t)) => t,
            Ok(None) => return Some(Ok(::shingetsu::valuevec![Value::Nil])),
            Err(e) => return Some(Err(e)),
        };
        if !matches!(key, Value::String(_)) {
            return Some(Ok(::shingetsu::valuevec![Value::Nil]));
        }
        match table.raw_get(key) {
            Ok(v) => Some(Ok(::shingetsu::valuevec![v])),
            Err(e) => Some(Err(e)),
        }
    }

    /// `__len` metamethod body.
    pub fn len<T: serde::Serialize>(this: &T) -> Result<ValueVec, VmError> {
        let n = match as_table(this)? {
            Some(t) => t.raw_len(),
            None => 0,
        };
        Ok(::shingetsu::valuevec![Value::Integer(n)])
    }

    /// `__pairs` metamethod body: returns `(iter_fn, nil, nil)`
    /// where `iter_fn` walks the serialized table's entries.
    pub fn pairs<T: serde::Serialize>(this: &T) -> Result<ValueVec, VmError> {
        let mut entries: Vec<(Value, Value)> = Vec::new();
        if let Some(table) = as_table(this)? {
            let mut key = Value::Nil;
            while let Some((k, v)) = table.next(&key)? {
                entries.push((k.clone(), v));
                key = k;
            }
        }

        let iter = ::std::sync::Arc::new(::std::sync::Mutex::new(entries.into_iter()));
        let iter_fn = Function::wrap(
            "serde_index_pairs",
            move |_state: Value, _control: Value| -> Result<ValueVec, VmError> {
                match iter
                    .lock()
                    .expect("serde_index pairs iterator poisoned")
                    .next()
                {
                    Some((k, v)) => Ok(::shingetsu::valuevec![k, v]),
                    None => Ok(::shingetsu::valuevec![Value::Nil, Value::Nil]),
                }
            },
        );

        Ok(::shingetsu::valuevec![
            Value::Function(iter_fn),
            Value::Nil,
            Value::Nil
        ])
    }
}
