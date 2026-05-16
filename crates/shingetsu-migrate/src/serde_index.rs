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
    use ::shingetsu::serde_bridge::value_from_json;
    use ::shingetsu::{Function, Value, ValueVec, VmError};

    fn to_json<T: serde::Serialize>(this: &T) -> Result<serde_json::Value, VmError> {
        serde_json::to_value(this).map_err(|e| VmError::HostError {
            name: ::std::string::String::new(),
            source: e.to_string().into(),
        })
    }

    fn key_str(key: &Value) -> Option<String> {
        match key {
            Value::String(b) => ::std::str::from_utf8(b.as_ref()).ok().map(|s| s.to_owned()),
            _ => None,
        }
    }

    /// `Userdata::index` fast-path body: field read from the serde
    /// representation.
    pub fn index<T: serde::Serialize>(
        this: &T,
        key: &Value,
    ) -> Option<Result<ValueVec, VmError>> {
        let json = match to_json(this) {
            Ok(j) => j,
            Err(e) => return Some(Err(e)),
        };
        let serde_json::Value::Object(map) = json else {
            return Some(Ok(::shingetsu::valuevec![Value::Nil]));
        };
        let Some(name) = key_str(key) else {
            return Some(Ok(::shingetsu::valuevec![Value::Nil]));
        };
        match map.get(&name) {
            Some(v) => match value_from_json(v.clone()) {
                Ok(val) => Some(Ok(::shingetsu::valuevec![val])),
                Err(e) => Some(Err(e)),
            },
            None => Some(Ok(::shingetsu::valuevec![Value::Nil])),
        }
    }

    /// `__len` metamethod body.
    pub fn len<T: serde::Serialize>(this: &T) -> Result<ValueVec, VmError> {
        let n = match to_json(this)? {
            serde_json::Value::Object(m) => m.len() as i64,
            serde_json::Value::Array(a) => a.len() as i64,
            serde_json::Value::String(s) => s.len() as i64,
            _ => 0,
        };
        Ok(::shingetsu::valuevec![Value::Integer(n)])
    }

    /// `__pairs` metamethod body: returns `(iter_fn, nil, nil)`
    /// where `iter_fn` walks the serialized object's entries.
    pub fn pairs<T: serde::Serialize>(this: &T) -> Result<ValueVec, VmError> {
        let entries: Vec<(Value, Value)> = match to_json(this)? {
            serde_json::Value::Object(map) => map
                .into_iter()
                .map(|(k, v)| Ok((Value::string(k), value_from_json(v)?)))
                .collect::<Result<Vec<_>, VmError>>()?,
            _ => Vec::new(),
        };

        let iter = ::std::sync::Arc::new(::std::sync::Mutex::new(entries.into_iter()));
        let iter_fn = Function::wrap(
            "serde_index_pairs",
            move |_state: Value, _control: Value| -> Result<ValueVec, VmError> {
                match iter.lock().expect("serde_index pairs iterator poisoned").next() {
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
