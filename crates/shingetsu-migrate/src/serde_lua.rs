//! Cross-engine `SerdeLua<T>` facade.
//!
//! Wraps any `Serialize + DeserializeOwned` type so it can cross
//! the lua boundary on whichever backend the host is running.
//!
//! - On the shingetsu backend, conversion delegates to
//!   [`shingetsu::SerdeLua`] (json-bridge round trip).
//! - On the mlua backend, conversion replicates kumomta's
//!   historical `config::SerdeWrappedValue` semantics exactly:
//!   `into_lua` uses `serialize_none_to_null(false)` /
//!   `serialize_unit_to_null(false)`; `from_lua` falls back to
//!   materializing userdata via its `__pairs` metamethod and
//!   emits the `", while processing"` error suffix that
//!   `typing.lua`'s regex depends on.
//!
//! After final removal this type collapses to
//! `shingetsu::SerdeLua` (the mlua impls drop with the backend).

pub struct SerdeLua<T>(pub T);

impl<T> SerdeLua<T> {
    pub fn new(value: T) -> Self {
        SerdeLua(value)
    }

    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> std::ops::Deref for SerdeLua<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> std::ops::DerefMut for SerdeLua<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<T: Clone> Clone for SerdeLua<T> {
    fn clone(&self) -> Self {
        SerdeLua(self.0.clone())
    }
}

impl<T: Default> Default for SerdeLua<T> {
    fn default() -> Self {
        SerdeLua(T::default())
    }
}

impl<T: serde::Serialize> serde::Serialize for SerdeLua<T> {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(s)
    }
}

// ---------------------------------------------------------------------------
// shingetsu backend
// ---------------------------------------------------------------------------

#[cfg(feature = "shingetsu-backend")]
impl<T: serde::de::DeserializeOwned> shingetsu::FromLua for SerdeLua<T> {
    fn from_lua(v: shingetsu::Value) -> Result<Self, shingetsu::VmError> {
        let inner = <shingetsu::SerdeLua<T> as shingetsu::FromLua>::from_lua(v)?;
        Ok(SerdeLua(inner.into_inner()))
    }
}

#[cfg(feature = "shingetsu-backend")]
impl<T: serde::Serialize> shingetsu::IntoLua for SerdeLua<T> {
    fn into_lua(self) -> shingetsu::Value {
        shingetsu::SerdeLua(self.0).into_lua()
    }
}

#[cfg(feature = "shingetsu-backend")]
impl<T> shingetsu::LuaTyped for SerdeLua<T> {
    fn lua_type() -> shingetsu::LuaType {
        <shingetsu::SerdeLua<T> as shingetsu::LuaTyped>::lua_type()
    }
}

// ---------------------------------------------------------------------------
// mlua backend — replicates kumomta's `config::SerdeWrappedValue`
// ---------------------------------------------------------------------------

#[cfg(feature = "mlua-backend")]
fn serialize_options() -> mlua::SerializeOptions {
    mlua::SerializeOptions::new()
        .serialize_none_to_null(false)
        .serialize_unit_to_null(false)
}

/// Obtain a native lua representation of `value` by recursively
/// materializing any userdata it contains through its `__pairs`
/// metamethod, so the result can be processed by serde's
/// `Deserialize` impl on `mlua::Value`.
#[cfg(feature = "mlua-backend")]
fn materialize_to_lua_value(lua: &mlua::Lua, value: mlua::Value) -> mlua::Result<mlua::Value> {
    match value {
        mlua::Value::UserData(ud) => {
            let mt = ud.metatable()?;
            let Ok(pairs) = mt.get::<mlua::Function>("__pairs") else {
                let value = mlua::IntoLua::into_lua(ud, lua)?;
                return Err(mlua::Error::external(format!(
                    "cannot materialize_to_lua_value {value:?} \
                     because it has no __pairs metamethod"
                )));
            };
            let tbl = lua.create_table()?;
            let (iter_func, state, mut control): (mlua::Function, mlua::Value, mlua::Value) =
                pairs.call(mlua::Value::UserData(ud.clone()))?;

            loop {
                let (k, v): (mlua::Value, mlua::Value) =
                    iter_func.call((state.clone(), control))?;
                if k.is_nil() {
                    break;
                }

                tbl.set(k.clone(), materialize_to_lua_value(lua, v)?)?;
                control = k;
            }

            Ok(mlua::Value::Table(tbl))
        }
        mlua::Value::Table(t) => {
            let tbl = lua.create_table()?;
            for pair in t.pairs::<mlua::Value, mlua::Value>() {
                let (k, v) = pair?;
                tbl.set(k.clone(), materialize_to_lua_value(lua, v)?)?;
            }
            Ok(mlua::Value::Table(tbl))
        }
        value => Ok(value),
    }
}

/// Convert from a lua value to a deserializable type, with a
/// slightly more helpful error message in case of failure.
///
/// NOTE: the `", while processing"` portion of the error messages
/// generated here is coupled with a regex in kumomta's
/// `typing.lua` and must not be reworded.
#[cfg(feature = "mlua-backend")]
fn from_lua_value<R>(lua: &mlua::Lua, value: mlua::Value) -> mlua::Result<R>
where
    R: serde::de::DeserializeOwned,
{
    use mlua::LuaSerdeExt;
    use serde::Serialize;

    let value_cloned = value.clone();
    match lua.from_value(value) {
        Ok(r) => Ok(r),
        Err(err) => match materialize_to_lua_value(lua, value_cloned.clone()) {
            Ok(materialized) => match lua.from_value(materialized.clone()) {
                Ok(r) => Ok(r),
                Err(err) => {
                    let mut serializer = serde_json::Serializer::new(Vec::new());
                    let serialized = match materialized.serialize(&mut serializer) {
                        Ok(_) => String::from_utf8_lossy(&serializer.into_inner()).to_string(),
                        Err(err) => format!("<unable to encode as json: {err:#}>"),
                    };
                    Err(mlua::Error::external(format!(
                        "{err:#}, while processing {serialized}"
                    )))
                }
            },
            Err(materialize_err) => Err(mlua::Error::external(format!(
                "{err:#}, while processing a userdata. \
                    Additionally, encountered {materialize_err:#} \
                    when trying to iterate the pairs of that userdata"
            ))),
        },
    }
}

#[cfg(feature = "mlua-backend")]
impl<T: serde::Serialize> SerdeLua<T> {
    pub fn to_lua_value(&self, lua: &mlua::Lua) -> mlua::Result<mlua::Value> {
        use mlua::LuaSerdeExt;
        lua.to_value_with(&self.0, serialize_options())
    }
}

#[cfg(feature = "mlua-backend")]
impl<T: serde::Serialize> mlua::IntoLua for SerdeLua<T> {
    fn into_lua(self, lua: &mlua::Lua) -> mlua::Result<mlua::Value> {
        use mlua::LuaSerdeExt;
        lua.to_value_with(&self.0, serialize_options())
    }
}

#[cfg(feature = "mlua-backend")]
impl<T: serde::de::DeserializeOwned> mlua::FromLua for SerdeLua<T> {
    fn from_lua(value: mlua::Value, lua: &mlua::Lua) -> mlua::Result<Self> {
        let inner: T = from_lua_value(lua, value)?;
        Ok(SerdeLua(inner))
    }
}
