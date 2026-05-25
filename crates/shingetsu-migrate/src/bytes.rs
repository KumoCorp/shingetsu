//! Cross-engine `Bytes` facade: a portable byte string.
//!
//! Hosts that historically took an `mlua::String` parameter or
//! returned `lua.create_string(&[u8])` (binary, possibly non-UTF-8)
//! can use `Bytes` instead so a single signature works on both
//! engines:
//!
//! - On the shingetsu backend, conversion delegates to
//!   [`shingetsu::Bytes`] (`Value::String` round trip).
//! - On the mlua backend, `from_lua` delegates to
//!   `mlua::String`'s own `FromLua` (preserving mlua's
//!   string/number coercion exactly) and `into_lua` builds a lua
//!   string via `lua.create_string`.
//!
//! After final removal this collapses to `shingetsu::Bytes` (the
//! mlua impls drop with the backend).

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Bytes(pub Vec<u8>);

impl Bytes {
    pub fn new(value: impl Into<Vec<u8>>) -> Self {
        Bytes(value.into())
    }

    pub fn into_inner(self) -> Vec<u8> {
        self.0
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

impl std::ops::Deref for Bytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl AsRef<[u8]> for Bytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<Vec<u8>> for Bytes {
    fn from(v: Vec<u8>) -> Self {
        Bytes(v)
    }
}

impl From<&[u8]> for Bytes {
    fn from(v: &[u8]) -> Self {
        Bytes(v.to_vec())
    }
}

impl From<String> for Bytes {
    fn from(v: String) -> Self {
        Bytes(v.into_bytes())
    }
}

impl From<&str> for Bytes {
    fn from(v: &str) -> Self {
        Bytes(v.as_bytes().to_vec())
    }
}

impl From<Bytes> for Vec<u8> {
    fn from(b: Bytes) -> Vec<u8> {
        b.0
    }
}

// ---------------------------------------------------------------------------
// shingetsu backend
// ---------------------------------------------------------------------------

#[cfg(feature = "shingetsu-backend")]
impl shingetsu::FromLua for Bytes {
    fn from_lua(
        v: shingetsu::Value,
        env: &shingetsu::GlobalEnv,
    ) -> Result<Self, shingetsu::VmError> {
        let inner = <shingetsu::Bytes as shingetsu::FromLua>::from_lua(v, env)?;
        Ok(Bytes((*inner).to_vec()))
    }
}

#[cfg(feature = "shingetsu-backend")]
impl shingetsu::IntoLua for Bytes {
    fn into_lua(self) -> shingetsu::Value {
        shingetsu::IntoLua::into_lua(shingetsu::Bytes::from(self.0))
    }
}

#[cfg(feature = "shingetsu-backend")]
impl shingetsu::LuaTyped for Bytes {
    fn lua_type() -> shingetsu::LuaType {
        <shingetsu::Bytes as shingetsu::LuaTyped>::lua_type()
    }
}

// ---------------------------------------------------------------------------
// mlua backend
// ---------------------------------------------------------------------------

#[cfg(feature = "mlua-backend")]
impl mlua::IntoLua for Bytes {
    fn into_lua(self, lua: &mlua::Lua) -> mlua::Result<mlua::Value> {
        Ok(mlua::Value::String(lua.create_string(&self.0)?))
    }
}

#[cfg(feature = "mlua-backend")]
impl mlua::FromLua for Bytes {
    fn from_lua(value: mlua::Value, lua: &mlua::Lua) -> mlua::Result<Self> {
        // Delegate to mlua::String so the host sees identical
        // string/number coercion to the pre-migration code.
        let s = <mlua::String as mlua::FromLua>::from_lua(value, lua)?;
        Ok(Bytes(s.as_bytes().to_vec()))
    }
}
