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
//! The bare, value-typed variadic (`Variadic<Value>`-shaped raw
//! inspection) and the kumomta pattern of "collect a multi and
//! convert to JSON" aren't yet supported through this bridge.

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
        FromLua, FromLuaMulti, IntoLua, IntoLuaMulti, LuaType, LuaTyped, ValueVec, VmError,
    };

    impl<T: IntoLua> IntoLuaMulti for Variadic<T> {
        fn into_lua_multi(self) -> ValueVec {
            self.0.into_iter().map(IntoLua::into_lua).collect()
        }
    }

    impl<T: FromLua> FromLuaMulti for Variadic<T> {
        fn from_lua_multi(values: ValueVec) -> Result<Self, VmError> {
            values
                .into_iter()
                .map(T::from_lua)
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
