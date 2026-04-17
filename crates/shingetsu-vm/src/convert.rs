use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use bytes::Bytes;

use crate::error::VmError;
use crate::function::Function;
use crate::table::Table;
use crate::types::{LuaType, ValueType};
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
        T::from_lua(values.into_iter().next().unwrap_or(Value::Nil)).map_err(|e| match e {
            VmError::BadArgument { expected, got, .. } => VmError::BadArgument {
                position: 1,
                function: String::new(),
                expected,
                got,
            },
            other => other,
        })
    }
}

// ---------------------------------------------------------------------------
// LuaTyped trait
// ---------------------------------------------------------------------------

/// Provides the [`LuaType`] metadata for a Rust type that bridges to Lua.
pub trait LuaTyped {
    fn lua_type() -> LuaType;

    /// Simplified runtime type for fast call-site validation.
    ///
    /// Returns `None` for unconstrained types (`Value`, `Option<T>`) where
    /// any value is acceptable.  The default implementation returns `None`;
    /// concrete types override this.
    fn value_type() -> Option<ValueType> {
        None
    }
}

// ---------------------------------------------------------------------------
// LuaTypedMulti trait
// ---------------------------------------------------------------------------

/// Provides [`LuaType`] metadata for a Rust type that implements
/// [`IntoLuaMulti`] (multi-return).
///
/// For single-valued types that implement [`LuaTyped`], the blanket impl
/// wraps the single type in a one-element vector.  Tuple types, `Variadic`,
/// and custom multi-return enums provide their own implementations.
pub trait LuaTypedMulti {
    fn lua_types() -> Vec<LuaType>;
}

/// Blanket: any single-valued `LuaTyped` type produces a one-element return
/// list.
impl<T: LuaTyped> LuaTypedMulti for T {
    fn lua_types() -> Vec<LuaType> {
        vec![T::lua_type()]
    }
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
    fn value_type() -> Option<ValueType> {
        Some(ValueType::Boolean)
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
    fn value_type() -> Option<ValueType> {
        Some(ValueType::Integer)
    }
}

// ---------------------------------------------------------------------------
// Never — uninhabited return type for functions that always error
// ---------------------------------------------------------------------------

/// Uninhabited type for functions that never return successfully.
///
/// Use as `Result<Never, VmError>` for functions like `error()` and
/// `os.exit()` that always produce a `VmError`.  The `IntoLua` and
/// `LuaTyped` impls exist only to satisfy trait bounds — they are
/// never called at runtime.
pub enum Never {}

impl IntoLua for Never {
    fn into_lua(self) -> Value {
        match self {}
    }
}

impl LuaTyped for Never {
    fn lua_type() -> LuaType {
        LuaType::Never
    }
}

// ---------------------------------------------------------------------------
// Number — integer-or-float Lua number
// ---------------------------------------------------------------------------

/// A Lua number value that preserves the integer/float distinction.
///
/// Use this as a return type for functions like `math.floor`,
/// `math.abs`, `tonumber`, etc. that return a number whose
/// integer/float subtype depends on the input or on overflow.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Number {
    Integer(i64),
    Float(f64),
}

impl Number {
    /// Convert to `f64`, losing the integer/float distinction.
    pub fn into_float(self) -> f64 {
        match self {
            Number::Integer(n) => n as f64,
            Number::Float(f) => f,
        }
    }
}

impl FromLua for Number {
    fn from_lua(v: Value) -> Result<Self, VmError> {
        match v {
            Value::Integer(n) => Ok(Number::Integer(n)),
            Value::Float(f) => Ok(Number::Float(f)),
            other => Err(VmError::BadArgument {
                position: 0,
                function: String::new(),
                expected: "number".to_owned(),
                got: other.type_name().to_owned(),
            }),
        }
    }
}

impl IntoLua for Number {
    fn into_lua(self) -> Value {
        match self {
            Number::Integer(n) => Value::Integer(n),
            Number::Float(f) => Value::Float(f),
        }
    }
}

impl LuaTyped for Number {
    fn lua_type() -> LuaType {
        LuaType::Number
    }
}

// ---------------------------------------------------------------------------
// CoerceInt — integer that accepts float coercion
// ---------------------------------------------------------------------------

/// An `i64` that accepts both `Value::Integer` and `Value::Float` via
/// `FromLua`, matching Lua's `luaL_checkinteger` coercion semantics.
///
/// A float value is accepted only if it is finite and has no fractional
/// part (i.e. it is an exact integer).  Non-integer floats are rejected.
///
/// Use this instead of `i64` when the Lua-facing API should accept both
/// `3` and `3.0` as equivalent integer arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CoerceInt(pub i64);

impl FromLua for CoerceInt {
    fn from_lua(v: Value) -> Result<Self, VmError> {
        match v {
            Value::Integer(n) => Ok(CoerceInt(n)),
            Value::Float(f) => {
                if f.is_finite() && f.fract() == 0.0 && f >= i64::MIN as f64 && f <= i64::MAX as f64
                {
                    Ok(CoerceInt(f as i64))
                } else {
                    Err(VmError::ArgError {
                        position: 0,
                        function: String::new(),
                        msg: "number has no integer representation".to_owned(),
                    })
                }
            }
            other => Err(VmError::BadArgument {
                position: 0,
                function: String::new(),
                expected: "number".to_owned(),
                got: other.type_name().to_owned(),
            }),
        }
    }
}

impl IntoLua for CoerceInt {
    fn into_lua(self) -> Value {
        Value::Integer(self.0)
    }
}

impl LuaTyped for CoerceInt {
    fn lua_type() -> LuaType {
        LuaType::Number
    }
    fn value_type() -> Option<ValueType> {
        Some(ValueType::Number)
    }
}

impl std::ops::Deref for CoerceInt {
    type Target = i64;
    fn deref(&self) -> &i64 {
        &self.0
    }
}

impl From<CoerceInt> for i64 {
    fn from(c: CoerceInt) -> i64 {
        c.0
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
    fn value_type() -> Option<ValueType> {
        Some(ValueType::Integer)
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
    fn value_type() -> Option<ValueType> {
        Some(ValueType::Integer)
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
    fn value_type() -> Option<ValueType> {
        Some(ValueType::Integer)
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
    fn value_type() -> Option<ValueType> {
        Some(ValueType::Number)
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
    fn value_type() -> Option<ValueType> {
        Some(ValueType::Number)
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
        Value::string(self)
    }
}

impl IntoLua for &str {
    fn into_lua(self) -> Value {
        Value::String(Bytes::copy_from_slice(self.as_bytes()))
    }
}

impl LuaTyped for &str {
    fn lua_type() -> LuaType {
        LuaType::String
    }
    fn value_type() -> Option<ValueType> {
        Some(ValueType::String)
    }
}

impl LuaTyped for String {
    fn lua_type() -> LuaType {
        LuaType::String
    }
    fn value_type() -> Option<ValueType> {
        Some(ValueType::String)
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
    fn value_type() -> Option<ValueType> {
        Some(ValueType::String)
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

impl LuaTypedMulti for () {
    fn lua_types() -> Vec<LuaType> {
        vec![]
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
    fn value_type() -> Option<ValueType> {
        Some(ValueType::Table)
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
    fn value_type() -> Option<ValueType> {
        Some(ValueType::Function)
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

impl LuaTyped for Arc<dyn Userdata> {
    fn lua_type() -> LuaType {
        LuaType::Any
    }

    fn value_type() -> Option<ValueType> {
        Some(ValueType::Userdata)
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
// StdlibResult — success/error return pattern
// ---------------------------------------------------------------------------

/// Return type for stdlib functions that return `T` on success or
/// `(nil, errmsg)` on failure.
///
/// This captures the common Lua idiom where functions like `io.open`
/// and `os.rename` return a value on success, or `nil` plus an error
/// message string on failure.  `pcall`-friendly: the error is a
/// normal return, not a thrown `VmError`.
///
/// ```rust,ignore
/// fn open(path: Bytes) -> Result<StdlibResult<LuaFile>, VmError> {
///     match do_open(&path) {
///         Ok(file) => Ok(StdlibResult::Ok(file)),
///         Err(msg) => Ok(StdlibResult::Err(msg)),
///     }
/// }
/// ```
pub enum StdlibResult<T: IntoLuaMulti = bool> {
    Ok(T),
    Err(String),
}

impl<T: IntoLuaMulti> IntoLuaMulti for StdlibResult<T> {
    fn into_lua_multi(self) -> Vec<Value> {
        match self {
            StdlibResult::Ok(v) => v.into_lua_multi(),
            StdlibResult::Err(msg) => vec![Value::Nil, Value::string(msg)],
        }
    }
}

impl<T: LuaTypedMulti + IntoLuaMulti> LuaTypedMulti for StdlibResult<T> {
    fn lua_types() -> Vec<LuaType> {
        let ok_types = T::lua_types();
        let ok_type = if ok_types.len() == 1 {
            ok_types.into_iter().next().expect("just checked len")
        } else {
            LuaType::Tuple(ok_types)
        };
        let err_type = LuaType::Tuple(vec![LuaType::Nil, LuaType::String]);
        vec![LuaType::Union(vec![ok_type, err_type])]
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

macro_rules! impl_lua_typed_multi_tuple {
    ($($name:ident)+) => {
        impl<$($name: LuaTyped,)*> LuaTypedMulti for ($($name,)*) {
            fn lua_types() -> Vec<LuaType> {
                vec![$($name::lua_type(),)*]
            }
        }
    };
}

impl_lua_typed_multi_tuple!(A);
impl_lua_typed_multi_tuple!(A B);
impl_lua_typed_multi_tuple!(A B C);
impl_lua_typed_multi_tuple!(A B C D);
impl_lua_typed_multi_tuple!(A B C D E);
impl_lua_typed_multi_tuple!(A B C D E F);
impl_lua_typed_multi_tuple!(A B C D E F G);
impl_lua_typed_multi_tuple!(A B C D E F G H);
impl_lua_typed_multi_tuple!(A B C D E F G H I);
impl_lua_typed_multi_tuple!(A B C D E F G H I J);
impl_lua_typed_multi_tuple!(A B C D E F G H I J K);
impl_lua_typed_multi_tuple!(A B C D E F G H I J K L);
impl_lua_typed_multi_tuple!(A B C D E F G H I J K L M);
impl_lua_typed_multi_tuple!(A B C D E F G H I J K L M N);
impl_lua_typed_multi_tuple!(A B C D E F G H I J K L M N O);
impl_lua_typed_multi_tuple!(A B C D E F G H I J K L M N O P);

macro_rules! impl_from_lua_multi {
    ($($name:ident)+) => {
        impl<$($name: FromLua,)*> FromLuaMulti for ($($name,)*) {
            #[allow(non_snake_case)]
            fn from_lua_multi(values: Vec<Value>) -> Result<Self, VmError> {
                let mut __iter = values.into_iter();
                let mut __pos: usize = 0;
                $(
                    __pos += 1;
                    let $name = $name::from_lua(__iter.next().unwrap_or(Value::Nil))
                        .map_err(|e| match e {
                            VmError::BadArgument { expected, got, .. } => VmError::BadArgument {
                                position: __pos,
                                function: String::new(),
                                expected,
                                got,
                            },
                            other => other,
                        })?;
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
