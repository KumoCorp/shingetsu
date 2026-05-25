//! Cross-engine userdata-borrow type for `#[lua_metamethod]` operands
//! (and other parameters that need to receive the same userdata type
//! from either engine).
//!
//! `UserDataRef<T>` decodes a `Value::Userdata` (shingetsu) or
//! `mlua::Value::UserData` (mlua) into a value that derefs to `&T`,
//! so a single metamethod body like
//!
//! ```ignore
//! #[lua_metamethod(Eq)]
//! fn eq(&self, other: UserDataRef<Self>) -> bool { *self == *other }
//! ```
//!
//! works under either backend.  The shingetsu variant holds an
//! `Arc<T>` (downcast from the type-erased `Arc<dyn Userdata>`); the
//! mlua variant holds an `mlua::UserDataRef<T>` (a runtime-checked
//! borrow against mlua's userdata table).

use std::ops::Deref;

#[cfg(feature = "shingetsu-backend")]
use std::sync::Arc;

/// Cross-engine borrow of a userdata value of concrete type `T`.
/// Constructed by the per-engine `FromLua` impls; deref to `&T`.
pub enum UserDataRef<T: 'static> {
    #[cfg(feature = "shingetsu-backend")]
    Shingetsu(Arc<T>),
    #[cfg(feature = "mlua-backend")]
    Mlua(mlua::UserDataRef<T>),
    // Hold the marker only when no backend is enabled so the enum
    // still has at least one variant.  In practice at least one
    // backend is always on.
    #[cfg(not(any(feature = "shingetsu-backend", feature = "mlua-backend")))]
    _Phantom(::std::marker::PhantomData<T>),
}

impl<T: 'static> Deref for UserDataRef<T> {
    type Target = T;
    fn deref(&self) -> &T {
        match self {
            #[cfg(feature = "shingetsu-backend")]
            Self::Shingetsu(arc) => arc.as_ref(),
            #[cfg(feature = "mlua-backend")]
            Self::Mlua(r) => r.deref(),
            #[cfg(not(any(feature = "shingetsu-backend", feature = "mlua-backend")))]
            Self::_Phantom(_) => unreachable!("no backend enabled"),
        }
    }
}

#[cfg(feature = "shingetsu-backend")]
mod shingetsu_impls {
    use super::UserDataRef;
    use shingetsu::{FromLua, GlobalEnv, LuaType, LuaTyped, Userdata, Value, VmError};
    use std::sync::Arc;

    impl<T: Userdata> FromLua for UserDataRef<T> {
        fn from_lua(v: Value, _env: &GlobalEnv) -> Result<Self, VmError> {
            match v {
                Value::Userdata(arc) => {
                    let got = arc.type_name().to_owned();
                    let arc: Arc<dyn Userdata> = arc;
                    arc.downcast_arc::<T>()
                        .map(UserDataRef::Shingetsu)
                        .map_err(|_| VmError::BadArgument {
                            position: 0,
                            function: String::new(),
                            expected: ::std::any::type_name::<T>().to_owned(),
                            got,
                        })
                }
                other => Err(VmError::BadArgument {
                    position: 0,
                    function: String::new(),
                    expected: "userdata".to_owned(),
                    got: other.type_name().to_owned(),
                }),
            }
        }
    }

    impl<T: Userdata + LuaTyped> LuaTyped for UserDataRef<T> {
        fn lua_type() -> LuaType {
            T::lua_type()
        }
    }
}

#[cfg(feature = "mlua-backend")]
mod mlua_impls {
    use super::UserDataRef;

    impl<T: 'static> mlua::FromLua for UserDataRef<T> {
        fn from_lua(v: mlua::Value, _lua: &mlua::Lua) -> mlua::Result<Self> {
            let got = v.type_name();
            match v {
                mlua::Value::UserData(ud) => ud.borrow::<T>().map(UserDataRef::Mlua),
                _ => Err(mlua::Error::FromLuaConversionError {
                    from: got,
                    to: ::std::any::type_name::<T>().to_owned(),
                    message: Some("expected userdata".to_owned()),
                }),
            }
        }
    }
}
