//! [`SerdeLua<T>`]: bridge wrapper that adapts any `Serialize +
//! DeserializeOwned` Rust type to lua via JSON as the intermediate
//! representation.
//!
//! Useful when:
//!
//! - The type has a manually-tuned serde representation (custom
//!   `try_from`/`into`, internally tagged, flattened) that you don't
//!   want to re-express as `derive(LuaRepr)` attributes.
//! - You're integrating a codebase that already uses serde extensively
//!   and converting types one at a time would be churn.
//! - The host caches the lua representation as JSON for reuse across
//!   VM contexts.
//!
//! For new code that doesn't already commit to serde, prefer
//! `derive(LuaRepr)` directly: it produces nicer error messages, is
//! visible to the type checker, and avoids the JSON intermediate.
//!
//! Conversion goes lua → `serde_json::Value` →
//! `T::deserialize` and `T::serialize` → `serde_json::Value` → lua.
//! See [`crate::serde_bridge`] for the lua/JSON edge cases.
//!
//! ## Fallibility
//!
//! [`FromLua`] returns a `Result`, so deserialization failures
//! (shape mismatches, type errors, custom serde validators) propagate
//! naturally as [`VmError::HostError`].
//!
//! [`IntoLua`] does **not** return a `Result` — its trait signature is
//! infallible.  Two failure modes therefore have nowhere to surface:
//!
//! 1. The serde `Serialize` impl returns an error (rare: most derived
//!    impls are infallible, but custom impls and adapters like
//!    `serde_with` can fail).
//! 2. The resulting `serde_json::Value` cannot be converted back to a
//!    lua `Value` (e.g. it contains an `f64::NAN` produced by serde).
//!
//! Rather than panic — which would be unsafe in stateful host
//! processes — these failures emit a diagnostic via the `log` crate
//! when the `log` cargo feature is enabled (route through your host's
//! existing logging infrastructure), or via `eprintln!` otherwise, and
//! return [`Value::Nil`].  Diagnostics include the Rust type name
//! (via [`std::any::type_name`]) and the underlying serde / bridge
//! error message so a missing field is identifiable in logs.
//!
//! `Nil` is chosen as the fallback because it is least likely to be
//! mistaken for valid data: most consuming lua code will treat a
//! missing/nil value as a clear signal that something went wrong,
//! whereas a stringified diagnostic embedded in a typed slot can
//! propagate silently.  Hosts that need stronger guarantees should
//! either avoid `Serialize` impls that can fail, or wrap their types
//! in `derive(LuaRepr)` (which surfaces failures through error
//! channels at construction time).

use crate::convert::{FromLua, IntoLua, LuaTyped};
use crate::error::VmError;
use crate::serde_bridge::value_to_json;
use crate::types::LuaType;
use crate::value::Value;

/// Wrapper that adapts a serde-friendly type to shingetsu's lua
/// conversion traits.  See module docs for usage and fallibility
/// caveats.
#[derive(Debug, Clone, PartialEq)]
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

impl<T: serde::de::DeserializeOwned> FromLua for SerdeLua<T> {
    fn from_lua(value: Value) -> Result<Self, VmError> {
        let json = value_to_json(&value)?;
        let inner: T = serde_json::from_value(json).map_err(|e| VmError::HostError {
            name: "SerdeLua::from_lua".to_owned(),
            source: e.to_string().into(),
        })?;
        Ok(SerdeLua(inner))
    }
}

impl<T: serde::Serialize> IntoLua for SerdeLua<T> {
    fn into_lua(self) -> Value {
        match crate::serde_ser::to_value(&self.0) {
            Ok(v) => v,
            Err(e) => {
                report_into_lua_failure::<T>("serialize", &e.to_string());
                Value::Nil
            }
        }
    }
}

fn report_into_lua_failure<T>(stage: &str, msg: &str) {
    let type_name = std::any::type_name::<T>();
    #[cfg(feature = "log")]
    {
        log::error!(
            "SerdeLua::into_lua: {stage} failure for {type_name}: {msg}; \
             returning Nil"
        );
    }
    #[cfg(not(feature = "log"))]
    {
        eprintln!(
            "SerdeLua::into_lua: {stage} failure for {type_name}: {msg}; \
             returning Nil"
        );
    }
}

impl<T> LuaTyped for SerdeLua<T> {
    fn lua_type() -> LuaType {
        // We can't derive structural type info from `T` without
        // running serde at compile time.  Surface as `Any` so the
        // type checker neither over- nor under-constrains call sites.
        // Hosts that want precise typing should use
        // `derive(LuaRepr)` instead.
        LuaType::Any
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Person {
        name: String,
        age: u32,
    }

    #[test]
    fn struct_round_trip_via_serde() {
        let p = SerdeLua::new(Person {
            name: "Alex".to_owned(),
            age: 42,
        });
        let lua = p.clone().into_lua();
        let back: SerdeLua<Person> = FromLua::from_lua(lua).expect("from_lua");
        k9::assert_equal!(back, p);
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(tag = "kind")]
    enum Shape {
        Circle { radius: f64 },
        Square { side: f64 },
    }

    #[test]
    fn serde_internally_tagged_enum_round_trips() {
        let s = SerdeLua::new(Shape::Circle { radius: 2.5 });
        let lua = s.clone().into_lua();
        let back: SerdeLua<Shape> = FromLua::from_lua(lua).expect("from_lua");
        k9::assert_equal!(back, s);
    }

    #[test]
    fn from_lua_reports_serde_error_on_shape_mismatch() {
        // Pass a string where Person expects a table.
        let err = <SerdeLua<Person>>::from_lua(Value::string("not a person")).expect_err("err");
        let rendered = format!("{err}");
        // The leaf error names the converter; the message comes from serde.
        assert!(
            rendered.starts_with("error in 'SerdeLua::from_lua':"),
            "unexpected error: {rendered}"
        );
    }

    #[test]
    fn lua_type_is_any() {
        // We don't introspect T at compile time; type-checker sees Any.
        match <SerdeLua<Person>>::lua_type() {
            LuaType::Any => {}
            other => panic!("expected Any, got {other:?}"),
        }
    }

    /// Custom Serialize impl that always fails.  Exercises the
    /// fallback path documented in the module's "Fallibility"
    /// section: the error is logged (see captured stderr below) and
    /// the returned lua value is `Nil`.
    struct AlwaysFails;
    impl Serialize for AlwaysFails {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("intentional test failure"))
        }
    }

    #[test]
    fn into_lua_falls_back_to_nil_on_serialize_failure() {
        let v = SerdeLua::new(AlwaysFails).into_lua();
        k9::assert_equal!(v, Value::Nil);
    }
}
