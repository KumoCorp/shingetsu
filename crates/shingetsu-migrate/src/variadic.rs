//! Cross-engine variadic parameter / return type for
//! `#[shingetsu_migrate::module]` and `#[shingetsu_migrate::userdata]`
//! function bodies.
//!
//! `Variadic<T>(pub Vec<T>)` carries a homogeneously-typed variadic
//! list and implements `FromLuaMulti` / `IntoLuaMulti` on whichever
//! engines are enabled.  A function body that takes
//! `args: Variadic<String>` (with `#[function(variadic)]`) compiles
//! on both engines and reads its values out of `args.0`.
//!
//! [`JsonVariadic`] covers the kumomta pattern of "collect a multi
//! and convert each arg to `serde_json::Value`" (mod-sqlite /
//! mod-redis / mod-http / mod-mimepart): it delegates per element
//! to [`crate::SerdeLua`]`<serde_json::Value>`, so the conversion
//! (including mlua userdata `__pairs` materialization) matches the
//! host's historical `from_lua_value` exactly on both engines.
//!
//! The bare, value-typed variadic (`Variadic<Value>`-shaped raw
//! inspection) is still intrinsically engine-aware and stays on
//! the engine-native macro.

/// Cross-engine variadic of homogeneously-typed values.  The inner
/// `Vec<T>` carries the decoded arguments in order.
#[derive(Debug, Clone, Default)]
pub struct Variadic<T>(pub Vec<T>);

impl<T> Variadic<T> {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn into_inner(self) -> Vec<T> {
        self.0
    }
}

impl<T> From<Vec<T>> for Variadic<T> {
    fn from(v: Vec<T>) -> Self {
        Self(v)
    }
}

impl<T> IntoIterator for Variadic<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

#[cfg(feature = "shingetsu-backend")]
mod shingetsu_impls {
    use super::Variadic;
    use shingetsu::{
        FromLua, FromLuaMulti, GlobalEnv, IntoLua, IntoLuaMulti, LuaType, LuaTyped, ValueVec,
        VmError,
    };

    impl<T: IntoLua> IntoLuaMulti for Variadic<T> {
        fn into_lua_multi(self) -> ValueVec {
            self.0.into_iter().map(IntoLua::into_lua).collect()
        }
    }

    impl<T: FromLua> FromLuaMulti for Variadic<T> {
        fn from_lua_multi(values: ValueVec, env: &GlobalEnv) -> Result<Self, VmError> {
            values
                .into_iter()
                .map(|v| T::from_lua(v, env))
                .collect::<Result<Vec<_>, _>>()
                .map(Variadic)
        }
    }

    impl<T: LuaTyped> LuaTyped for Variadic<T> {
        fn lua_type() -> LuaType {
            LuaType::Variadic(Box::new(T::lua_type()))
        }
    }
}

/// A variadic whose arguments are each converted to
/// `serde_json::Value` at the lua boundary, mirroring the host's
/// `from_lua_value`/`multi_value_to_json_value` pattern.  Read the
/// decoded values out of `.0` (or [`JsonVariadic::into_inner`]); a
/// single body works on both engines without touching
/// engine-native `Value` types.
#[derive(Debug, Clone, Default)]
pub struct JsonVariadic(pub Vec<serde_json::Value>);

impl JsonVariadic {
    pub fn into_inner(self) -> Vec<serde_json::Value> {
        self.0
    }
}

impl IntoIterator for JsonVariadic {
    type Item = serde_json::Value;
    type IntoIter = std::vec::IntoIter<serde_json::Value>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

#[cfg(feature = "shingetsu-backend")]
mod json_variadic_shingetsu {
    use super::JsonVariadic;
    use crate::SerdeLua;
    use shingetsu::{FromLua, FromLuaMulti, GlobalEnv, LuaType, LuaTyped, ValueVec, VmError};

    impl FromLuaMulti for JsonVariadic {
        fn from_lua_multi(values: ValueVec, env: &GlobalEnv) -> Result<Self, VmError> {
            values
                .into_iter()
                .map(|v| {
                    <SerdeLua<serde_json::Value> as FromLua>::from_lua(v, env)
                        .map(SerdeLua::into_inner)
                })
                .collect::<Result<Vec<_>, _>>()
                .map(JsonVariadic)
        }
    }

    impl LuaTyped for JsonVariadic {
        fn lua_type() -> LuaType {
            LuaType::Variadic(Box::new(
                <SerdeLua<serde_json::Value> as LuaTyped>::lua_type(),
            ))
        }
    }
}

#[cfg(feature = "mlua-backend")]
mod json_variadic_mlua {
    use super::JsonVariadic;
    use crate::SerdeLua;

    impl mlua::FromLuaMulti for JsonVariadic {
        fn from_lua_multi(mut values: mlua::MultiValue, lua: &mlua::Lua) -> mlua::Result<Self> {
            values
                .drain(..)
                .map(|v| {
                    <SerdeLua<serde_json::Value> as mlua::FromLua>::from_lua(v, lua)
                        .map(SerdeLua::into_inner)
                })
                .collect::<mlua::Result<Vec<_>>>()
                .map(JsonVariadic)
        }
    }
}

#[cfg(feature = "mlua-backend")]
mod mlua_impls {
    use super::Variadic;

    impl<T: mlua::FromLua> mlua::FromLuaMulti for Variadic<T> {
        fn from_lua_multi(mut values: mlua::MultiValue, lua: &mlua::Lua) -> mlua::Result<Self> {
            values
                .drain(..)
                .map(|v| T::from_lua(v, lua))
                .collect::<mlua::Result<Vec<_>>>()
                .map(Variadic)
        }
    }

    impl<T: mlua::IntoLua> mlua::IntoLuaMulti for Variadic<T> {
        fn into_lua_multi(self, lua: &mlua::Lua) -> mlua::Result<mlua::MultiValue> {
            let mut out = mlua::MultiValue::new();
            for item in self.0 {
                out.push_back(item.into_lua(lua)?);
            }
            Ok(out)
        }
    }
}
