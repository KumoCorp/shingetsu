use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use bytes::Bytes;

use crate::error::VmError;
use crate::function::Function;
use crate::table::Table;
use crate::types::LuaType;
use crate::userdata::Userdata;
use crate::value::Value;

// ---------------------------------------------------------------------------
// Variadic newtype
// ---------------------------------------------------------------------------

/// A variadic argument or return list.
///
/// As a function parameter, `Variadic` collects all remaining arguments from
/// the current position onward.  It must be the **last** parameter.
///
/// As a return type, `Variadic` passes its contents through as multiple return
/// values.
#[derive(Debug, Clone, Default)]
pub struct Variadic(pub Vec<Value>);

// ---------------------------------------------------------------------------
// Core conversion traits
// ---------------------------------------------------------------------------

/// Convert a single Lua [`Value`] into a Rust type.
pub trait FromLua: Sized {
    fn from_lua(v: Value) -> Result<Self, VmError>;
}

/// Convert a Rust value into a single Lua [`Value`].
pub trait IntoLua {
    fn into_lua(self) -> Value;
}

/// Convert a Rust value into a (possibly multi-valued) Lua return list.
pub trait IntoLuaMulti {
    fn into_lua_multi(self) -> Vec<Value>;
}

/// Blanket: any `IntoLua` type is also an `IntoLuaMulti` (singleton list).
impl<T: IntoLua> IntoLuaMulti for T {
    fn into_lua_multi(self) -> Vec<Value> {
        vec![self.into_lua()]
    }
}

/// Convert a (possibly multi-valued) Lua return list into a Rust type.
///
/// This is the inverse of [`IntoLuaMulti`].  It is implemented for:
/// - any type that implements [`FromLua`] (extracts the first value, or `nil`
///   when the list is empty),
/// - [`Variadic`] (wraps the whole list unchanged),
/// - tuples up to arity 16 (extracts positionally, `nil`-padding short lists).
pub trait FromLuaMulti: Sized {
    fn from_lua_multi(values: Vec<Value>) -> Result<Self, VmError>;
}

/// Blanket: any `FromLua` type extracts the first return value (or `nil`).
impl<T: FromLua> FromLuaMulti for T {
    fn from_lua_multi(values: Vec<Value>) -> Result<Self, VmError> {
        T::from_lua(values.into_iter().next().unwrap_or(Value::Nil))
    }
}

// ---------------------------------------------------------------------------
// LuaTyped trait
// ---------------------------------------------------------------------------

/// Provides the [`LuaType`] metadata for a Rust type that bridges to Lua.
pub trait LuaTyped {
    fn lua_type() -> LuaType;
}

// ---------------------------------------------------------------------------
// Primitive impls
// ---------------------------------------------------------------------------

impl FromLua for bool {
    fn from_lua(v: Value) -> Result<Self, VmError> {
        match v {
            Value::Boolean(b) => Ok(b),
            other => Err(VmError::BadArgument {
                position: 0,
                function: String::new(),
                expected: "boolean".to_owned(),
                got: other.type_name().to_owned(),
            }),
        }
    }
}

impl IntoLua for bool {
    fn into_lua(self) -> Value {
        Value::Boolean(self)
    }
}

impl LuaTyped for bool {
    fn lua_type() -> LuaType {
        LuaType::Boolean
    }
}

impl FromLua for i64 {
    fn from_lua(v: Value) -> Result<Self, VmError> {
        match v {
            Value::Integer(n) => Ok(n),
            other => Err(VmError::BadArgument {
                position: 0,
                function: String::new(),
                expected: "integer".to_owned(),
                got: other.type_name().to_owned(),
            }),
        }
    }
}

impl IntoLua for i64 {
    fn into_lua(self) -> Value {
        Value::Integer(self)
    }
}

impl LuaTyped for i64 {
    fn lua_type() -> LuaType {
        LuaType::Integer
    }
}

impl FromLua for i32 {
    fn from_lua(v: Value) -> Result<Self, VmError> {
        let n = i64::from_lua(v)?;
        i32::try_from(n).map_err(|_| VmError::BadArgument {
            position: 0,
            function: String::new(),
            expected: "integer (i32 range)".to_owned(),
            got: n.to_string(),
        })
    }
}

impl IntoLua for i32 {
    fn into_lua(self) -> Value {
        Value::Integer(self as i64)
    }
}

impl LuaTyped for i32 {
    fn lua_type() -> LuaType {
        LuaType::Integer
    }
}

impl FromLua for u32 {
    fn from_lua(v: Value) -> Result<Self, VmError> {
        let n = i64::from_lua(v)?;
        u32::try_from(n).map_err(|_| VmError::BadArgument {
            position: 0,
            function: String::new(),
            expected: "integer (u32 range)".to_owned(),
            got: n.to_string(),
        })
    }
}

impl IntoLua for u32 {
    fn into_lua(self) -> Value {
        Value::Integer(self as i64)
    }
}

impl LuaTyped for u32 {
    fn lua_type() -> LuaType {
        LuaType::Integer
    }
}

impl FromLua for usize {
    fn from_lua(v: Value) -> Result<Self, VmError> {
        let n = i64::from_lua(v)?;
        usize::try_from(n).map_err(|_| VmError::BadArgument {
            position: 0,
            function: String::new(),
            expected: "non-negative integer".to_owned(),
            got: n.to_string(),
        })
    }
}

impl IntoLua for usize {
    fn into_lua(self) -> Value {
        Value::Integer(self as i64)
    }
}

impl LuaTyped for usize {
    fn lua_type() -> LuaType {
        LuaType::Integer
    }
}

impl FromLua for f64 {
    fn from_lua(v: Value) -> Result<Self, VmError> {
        match v {
            Value::Float(f) => Ok(f),
            Value::Integer(n) => Ok(n as f64),
            other => Err(VmError::BadArgument {
                position: 0,
                function: String::new(),
                expected: "number".to_owned(),
                got: other.type_name().to_owned(),
            }),
        }
    }
}

impl IntoLua for f64 {
    fn into_lua(self) -> Value {
        Value::Float(self)
    }
}

impl LuaTyped for f64 {
    fn lua_type() -> LuaType {
        LuaType::Float
    }
}

impl FromLua for f32 {
    fn from_lua(v: Value) -> Result<Self, VmError> {
        Ok(f64::from_lua(v)? as f32)
    }
}

impl IntoLua for f32 {
    fn into_lua(self) -> Value {
        Value::Float(self as f64)
    }
}

impl LuaTyped for f32 {
    fn lua_type() -> LuaType {
        LuaType::Float
    }
}

impl FromLua for String {
    fn from_lua(v: Value) -> Result<Self, VmError> {
        match v {
            Value::String(s) => Ok(String::from_utf8_lossy(&s).into_owned()),
            other => Err(VmError::BadArgument {
                position: 0,
                function: String::new(),
                expected: "string".to_owned(),
                got: other.type_name().to_owned(),
            }),
        }
    }
}

impl IntoLua for String {
    fn into_lua(self) -> Value {
        Value::String(Bytes::from(self))
    }
}

impl IntoLua for &str {
    fn into_lua(self) -> Value {
        Value::String(Bytes::copy_from_slice(self.as_bytes()))
    }
}

impl LuaTyped for String {
    fn lua_type() -> LuaType {
        LuaType::String
    }
}

impl FromLua for Bytes {
    fn from_lua(v: Value) -> Result<Self, VmError> {
        match v {
            Value::String(s) => Ok(s),
            other => Err(VmError::BadArgument {
                position: 0,
                function: String::new(),
                expected: "string".to_owned(),
                got: other.type_name().to_owned(),
            }),
        }
    }
}

impl IntoLua for Bytes {
    fn into_lua(self) -> Value {
        Value::String(self)
    }
}

impl LuaTyped for Bytes {
    fn lua_type() -> LuaType {
        LuaType::String
    }
}

/// `()` represents an empty return list — zero Lua values.
///
/// `()` deliberately does NOT implement `IntoLua` (which would produce a single
/// `nil` value).  Instead it only implements `IntoLuaMulti`, producing an empty
/// `Vec<Value>`, so that Rust functions returning `()` yield zero Lua returns.
impl FromLua for () {
    fn from_lua(_v: Value) -> Result<Self, VmError> {
        Ok(())
    }
}

impl IntoLuaMulti for () {
    fn into_lua_multi(self) -> Vec<Value> {
        vec![]
    }
}

impl LuaTyped for () {
    fn lua_type() -> LuaType {
        LuaType::Tuple(vec![])
    }
}

/// Identity: `Value` passes through unchanged.
impl FromLua for Value {
    fn from_lua(v: Value) -> Result<Self, VmError> {
        Ok(v)
    }
}

impl IntoLua for Value {
    fn into_lua(self) -> Value {
        self
    }
}

impl LuaTyped for Value {
    fn lua_type() -> LuaType {
        LuaType::Any
    }
}

// ---------------------------------------------------------------------------
// Table and Function
// ---------------------------------------------------------------------------

impl FromLua for Table {
    fn from_lua(v: Value) -> Result<Self, VmError> {
        match v {
            Value::Table(t) => Ok(t),
            other => Err(VmError::BadArgument {
                position: 0,
                function: String::new(),
                expected: "table".to_owned(),
                got: other.type_name().to_owned(),
            }),
        }
    }
}

impl IntoLua for Table {
    fn into_lua(self) -> Value {
        Value::Table(self)
    }
}

impl LuaTyped for Table {
    fn lua_type() -> LuaType {
        LuaType::Table(Box::new(crate::types::TableLuaType {
            fields: vec![],
            indexer: None,
        }))
    }
}

impl FromLua for Function {
    fn from_lua(v: Value) -> Result<Self, VmError> {
        match v {
            Value::Function(f) => Ok(f),
            other => Err(VmError::BadArgument {
                position: 0,
                function: String::new(),
                expected: "function".to_owned(),
                got: other.type_name().to_owned(),
            }),
        }
    }
}

impl IntoLua for Function {
    fn into_lua(self) -> Value {
        Value::Function(self)
    }
}

impl LuaTyped for Function {
    fn lua_type() -> LuaType {
        LuaType::Function(Box::new(crate::types::FunctionLuaType {
            type_params: vec![],
            params: vec![],
            variadic: Some(Box::new(LuaType::Any)),
            returns: vec![],
        }))
    }
}

// ---------------------------------------------------------------------------
// Userdata
// ---------------------------------------------------------------------------

impl FromLua for Arc<dyn Userdata> {
    fn from_lua(v: Value) -> Result<Self, VmError> {
        match v {
            Value::Userdata(u) => Ok(u),
            other => Err(VmError::BadArgument {
                position: 0,
                function: String::new(),
                expected: "userdata".to_owned(),
                got: other.type_name().to_owned(),
            }),
        }
    }
}

impl IntoLua for Arc<dyn Userdata> {
    fn into_lua(self) -> Value {
        Value::Userdata(self)
    }
}

// ---------------------------------------------------------------------------
// Option<T>
// ---------------------------------------------------------------------------

impl<T: FromLua> FromLua for Option<T> {
    fn from_lua(v: Value) -> Result<Self, VmError> {
        match v {
            Value::Nil => Ok(None),
            other => T::from_lua(other).map(Some),
        }
    }
}

impl<T: IntoLua> IntoLua for Option<T> {
    fn into_lua(self) -> Value {
        match self {
            Some(v) => v.into_lua(),
            None => Value::Nil,
        }
    }
}

impl<T: LuaTyped> LuaTyped for Option<T> {
    fn lua_type() -> LuaType {
        LuaType::Optional(Box::new(T::lua_type()))
    }
}

// ---------------------------------------------------------------------------
// Vec<T> — ipairs-style sequence table
// ---------------------------------------------------------------------------

impl<T: FromLua> FromLua for Vec<T> {
    fn from_lua(v: Value) -> Result<Self, VmError> {
        let table = Table::from_lua(v)?;
        let len = table.raw_len() as usize;
        let mut out = Vec::with_capacity(len);
        for i in 1..=len {
            let val = table.raw_get(&Value::Integer(i as i64))?;
            out.push(T::from_lua(val)?);
        }
        Ok(out)
    }
}

impl<T: IntoLua> IntoLua for Vec<T> {
    fn into_lua(self) -> Value {
        let table = Table::new();
        for (i, v) in self.into_iter().enumerate() {
            // Integer keys cannot be nil or NaN, so raw_set cannot fail here.
            let _ = table.raw_set(Value::Integer((i + 1) as i64), v.into_lua());
        }
        Value::Table(table)
    }
}

impl<T: LuaTyped> LuaTyped for Vec<T> {
    fn lua_type() -> LuaType {
        LuaType::Table(Box::new(crate::types::TableLuaType {
            fields: vec![],
            indexer: Some((Box::new(LuaType::Integer), Box::new(T::lua_type()))),
        }))
    }
}

// ---------------------------------------------------------------------------
// HashMap<K, V> and BTreeMap<K, V>
// ---------------------------------------------------------------------------

impl<K, V> FromLua for HashMap<K, V>
where
    K: FromLua + Eq + std::hash::Hash,
    V: FromLua,
{
    fn from_lua(v: Value) -> Result<Self, VmError> {
        let table = Table::from_lua(v)?;
        let mut out = HashMap::new();
        let mut key = Value::Nil;
        while let Some((k, val)) = table.next(&key)? {
            key = k.clone();
            out.insert(K::from_lua(k)?, V::from_lua(val)?);
        }
        Ok(out)
    }
}

impl<K, V> FromLua for BTreeMap<K, V>
where
    K: FromLua + Ord,
    V: FromLua,
{
    fn from_lua(v: Value) -> Result<Self, VmError> {
        let table = Table::from_lua(v)?;
        let mut out = BTreeMap::new();
        let mut key = Value::Nil;
        while let Some((k, val)) = table.next(&key)? {
            key = k.clone();
            out.insert(K::from_lua(k)?, V::from_lua(val)?);
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Variadic
// ---------------------------------------------------------------------------

impl IntoLuaMulti for Variadic {
    fn into_lua_multi(self) -> Vec<Value> {
        self.0
    }
}

/// `Variadic` collects the entire return list unchanged.
impl FromLuaMulti for Variadic {
    fn from_lua_multi(values: Vec<Value>) -> Result<Self, VmError> {
        Ok(Variadic(values))
    }
}

impl LuaTyped for Variadic {
    fn lua_type() -> LuaType {
        LuaType::Variadic(Box::new(LuaType::Any))
    }
}

// ---------------------------------------------------------------------------
// Tuple IntoLuaMulti / FromLuaMulti impls (up to arity 16)
// ---------------------------------------------------------------------------

macro_rules! impl_into_lua_multi {
    ($($name:ident)+) => {
        impl<$($name: IntoLua,)*> IntoLuaMulti for ($($name,)*) {
            #[allow(non_snake_case)]
            fn into_lua_multi(self) -> Vec<Value> {
                let ($($name,)*) = self;
                vec![$($name.into_lua(),)*]
            }
        }
    };
}

impl_into_lua_multi!(A);
impl_into_lua_multi!(A B);
impl_into_lua_multi!(A B C);
impl_into_lua_multi!(A B C D);
impl_into_lua_multi!(A B C D E);
impl_into_lua_multi!(A B C D E F);
impl_into_lua_multi!(A B C D E F G);
impl_into_lua_multi!(A B C D E F G H);
impl_into_lua_multi!(A B C D E F G H I);
impl_into_lua_multi!(A B C D E F G H I J);
impl_into_lua_multi!(A B C D E F G H I J K);
impl_into_lua_multi!(A B C D E F G H I J K L);
impl_into_lua_multi!(A B C D E F G H I J K L M);
impl_into_lua_multi!(A B C D E F G H I J K L M N);
impl_into_lua_multi!(A B C D E F G H I J K L M N O);
impl_into_lua_multi!(A B C D E F G H I J K L M N O P);

macro_rules! impl_from_lua_multi {
    ($($name:ident)+) => {
        impl<$($name: FromLua,)*> FromLuaMulti for ($($name,)*) {
            #[allow(non_snake_case)]
            fn from_lua_multi(values: Vec<Value>) -> Result<Self, VmError> {
                let mut __iter = values.into_iter();
                $(
                    let $name = $name::from_lua(__iter.next().unwrap_or(Value::Nil))?;
                )*
                Ok(($($name,)*))
            }
        }
    };
}

impl_from_lua_multi!(A);
impl_from_lua_multi!(A B);
impl_from_lua_multi!(A B C);
impl_from_lua_multi!(A B C D);
impl_from_lua_multi!(A B C D E);
impl_from_lua_multi!(A B C D E F);
impl_from_lua_multi!(A B C D E F G);
impl_from_lua_multi!(A B C D E F G H);
impl_from_lua_multi!(A B C D E F G H I);
impl_from_lua_multi!(A B C D E F G H I J);
impl_from_lua_multi!(A B C D E F G H I J K);
impl_from_lua_multi!(A B C D E F G H I J K L);
impl_from_lua_multi!(A B C D E F G H I J K L M);
impl_from_lua_multi!(A B C D E F G H I J K L M N);
impl_from_lua_multi!(A B C D E F G H I J K L M N O);
impl_from_lua_multi!(A B C D E F G H I J K L M N O P);
