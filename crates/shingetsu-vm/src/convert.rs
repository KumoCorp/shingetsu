use crate::valuevec;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::byte_string::Bytes;
use crate::error::VmError;
use crate::function::Function;
use crate::table::Table;
use crate::types::{LuaType, ValueType};
use crate::userdata::Userdata;
use crate::value::{Value, ValueVec};

/// Standard `help:` annotation appended to "number has no integer
/// representation" errors.  These fire when a float that isn't an
/// exact integer (e.g. `3.5` or `1e100`) is passed where an integer
/// was expected.
const NO_INT_REP_HELP: &str = "floor, round, or truncate the value first \
                               (e.g. via `math.floor`, `math.tointeger`, \
                               or `//1`)";

// ---------------------------------------------------------------------------
// Variadic newtype
// ---------------------------------------------------------------------------

/// A variadic argument or return list wrapping a [`ValueVec`].
///
/// As a function parameter, `Variadic` collects all remaining arguments from
/// the current position onward.  It must be the **last** parameter.
///
/// As a return type, `Variadic` passes its contents through as multiple return
/// values.
///
/// The inner field is a [`ValueVec`] (`SmallVec<[Value; 3]>`).
#[derive(Debug, Clone, Default)]
pub struct Variadic(pub ValueVec);

// ---------------------------------------------------------------------------
// Core conversion traits
// ---------------------------------------------------------------------------

/// Convert a single Lua [`Value`] into a Rust type.
///
/// Can be derived with `#[derive(shingetsu::FromLua)]` for structs (converts
/// from a Lua table) and enums (tries each variant's inner type in order).
pub trait FromLua: Sized {
    fn from_lua(v: Value) -> Result<Self, VmError>;

    /// Extract from a borrowed `&Value`, avoiding a full `Value::clone()`
    /// when possible.  The default clones and delegates to [`Self::from_lua`];
    /// primitive types override this to copy the inner scalar directly.
    fn from_lua_ref(v: &Value) -> Result<Self, VmError> {
        Self::from_lua(v.clone())
    }
}

/// Extract a borrowed reference from a `&'a Value` without cloning.
///
/// This is the lifetime-parameterized counterpart of [`FromLua::from_lua_ref`].
/// It enables zero-copy extraction for types that can borrow directly from the
/// `Value` storage — primarily concrete `Userdata` types accessed via `&'a T`
/// or `&'a Arc<T>`.
///
/// Used by `#[function]` and `#[userdata]` proc macro codegen when a parameter
/// type is a reference.  Not used by `Function::wrap`, which stays on
/// `FromLua::from_lua_ref`.
pub trait FromLuaBorrow<'a>: Sized {
    fn from_lua_borrow(v: &'a Value) -> Result<Self, VmError>;
}

/// Convert a Rust value into a single Lua [`Value`].
///
/// Can be derived with `#[derive(shingetsu::IntoLua)]` for structs (converts
/// to a Lua table) and enums (delegates to the inner type).
pub trait IntoLua {
    fn into_lua(self) -> Value;
}

/// Marker trait: implementing types are guaranteed to produce a
/// [`Value::Table`] from their [`IntoLua`] impl.
///
/// Required as the inner type for internally-tagged enum variants
/// (`#[lua(tag = "...")]`), where the macro adds the tag to the
/// resulting table and therefore cannot operate on a non-table value.
///
/// Auto-implemented by:
/// - `Table` itself.
/// - `derive(IntoLua)` on structs without container `into` / `try_from`
///   (their `into_lua` always emits `Value::Table(...)`).
/// - `derive(IntoLua)` on internally- and adjacently-tagged enums.
///
/// Hand-written implementations must uphold the invariant; violating
/// it will trigger an `unreachable!` at runtime.
pub trait LuaTableShape: IntoLua {
    /// Convert directly to a [`Table`] without re-discriminating.
    ///
    /// The default implementation calls [`IntoLua::into_lua`] and
    /// asserts the result is a [`Value::Table`] — violating the
    /// trait's contract panics with `unreachable!`.  Implementations
    /// that already have a `Table` on hand should override this to
    /// avoid the redundant `into_lua`/match round trip.
    fn into_lua_table(self) -> Table
    where
        Self: Sized,
    {
        match self.into_lua() {
            Value::Table(t) => t,
            other => unreachable!("LuaTableShape contract violated: into_lua produced {other:?}"),
        }
    }
}

impl LuaTableShape for Table {
    fn into_lua_table(self) -> Table {
        self
    }
}

/// Convert a Rust value into a (possibly multi-valued) Lua return list.
///
/// Can be derived with `#[derive(shingetsu::IntoLuaMulti)]` for enums where
/// each variant represents a distinct multi-return shape.
pub trait IntoLuaMulti {
    fn into_lua_multi(self) -> ValueVec;
}

/// Blanket: any `IntoLua` type is also an `IntoLuaMulti` (singleton list).
impl<T: IntoLua> IntoLuaMulti for T {
    fn into_lua_multi(self) -> ValueVec {
        valuevec![self.into_lua()]
    }
}

/// Convert a (possibly multi-valued) Lua return list into a Rust type.
///
/// This is the inverse of [`IntoLuaMulti`].  It is implemented for:
/// - any type that implements [`FromLua`] (extracts the first value, or `nil`
///   when the list is empty),
/// - [`Variadic`] (wraps the whole list unchanged),
/// - tuples up to arity 16 (extracts positionally, `nil`-padding short lists).
///
/// Can be derived with `#[derive(shingetsu::FromLuaMulti)]` for enums where
/// each variant represents a distinct argument arity.
pub trait FromLuaMulti: Sized {
    fn from_lua_multi(values: ValueVec) -> Result<Self, VmError>;
}

/// Blanket: any `FromLua` type extracts the first return value (or `nil`).
impl<T: FromLua> FromLuaMulti for T {
    fn from_lua_multi(values: ValueVec) -> Result<Self, VmError> {
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
///
/// Can be derived with `#[derive(shingetsu::LuaTyped)]`.  For structs that
/// also implement [`FromLua`] and [`IntoLua`], prefer
/// `#[derive(shingetsu::LuaTable)]` which derives all three at once.
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

    /// Optional per-position parameter names for overload-dispatch
    /// enums.  Used by the `#[function]` macro to attach readable
    /// names to the synthesised parameter list when an enum-typed
    /// `Variadic` argument unpacks across multiple positions.
    ///
    /// Returns `None` at any position whose name is unknown or
    /// inconsistent across overload variants.  An empty vector
    /// (the default) means "no names available".
    fn lua_param_names() -> Vec<Option<&'static str>> {
        Vec::new()
    }
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

    fn from_lua_ref(v: &Value) -> Result<Self, VmError> {
        match v {
            Value::Boolean(b) => Ok(*b),
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
            Value::Float(f) => {
                if f.is_finite() && f.fract() == 0.0 && f >= i64::MIN as f64 && f <= i64::MAX as f64
                {
                    Ok(f as i64)
                } else {
                    Err(VmError::ArgError {
                        position: 0,
                        function: String::new(),
                        msg: "number has no integer representation".to_owned(),
                    }
                    .with_hint(NO_INT_REP_HELP))
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

    fn from_lua_ref(v: &Value) -> Result<Self, VmError> {
        match v {
            Value::Integer(n) => Ok(*n),
            Value::Float(f) => {
                let f = *f;
                if f.is_finite() && f.fract() == 0.0 && f >= i64::MIN as f64 && f <= i64::MAX as f64
                {
                    Ok(f as i64)
                } else {
                    Err(VmError::ArgError {
                        position: 0,
                        function: String::new(),
                        msg: "number has no integer representation".to_owned(),
                    }
                    .with_hint(NO_INT_REP_HELP))
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

impl IntoLua for i64 {
    fn into_lua(self) -> Value {
        Value::Integer(self)
    }
}

impl LuaTyped for i64 {
    fn lua_type() -> LuaType {
        LuaType::Number
    }
    fn value_type() -> Option<ValueType> {
        Some(ValueType::Number)
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
///
/// Functions returning `Never` are typed as `-> never` in the Lua
/// type system.  The type checker recognizes calls to such functions
/// as diverging, so they satisfy the missing-return analysis:
///
/// ```lua
/// -- No missing-return warning because reject() -> never
/// local function handler(): string
///     reject("forbidden")
/// end
/// ```
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

    /// Parse a sequence of hex digits as a 64-bit integer, wrapping
    /// modularly per Lua 5.4 §3.1: a hex integer literal "can be
    /// used to represent any 64-bit integer value, as the value is
    /// read in two's complement".  Both `0xFFFFFFFFFFFFFFFF` (16
    /// digits) and `0x13121110090807060504030201` (26 digits) parse
    /// successfully — the former to `-1`, the latter to the low 64
    /// bits as a signed integer.
    ///
    /// Returns `None` if `hex` is empty or contains a non-hex byte.
    pub fn parse_hex_integer_wrapping(hex: &str) -> Option<i64> {
        if hex.is_empty() {
            return None;
        }
        let mut acc: u64 = 0;
        for b in hex.bytes() {
            let digit = match b {
                b'0'..=b'9' => b - b'0',
                b'a'..=b'f' => b - b'a' + 10,
                b'A'..=b'F' => b - b'A' + 10,
                _ => return None,
            };
            acc = acc.wrapping_shl(4) | digit as u64;
        }
        Some(acc as i64)
    }

    /// Parse a string as a Lua numeric literal.  Recognises:
    /// * Decimal integers (with optional sign).
    /// * Hexadecimal integers (`0x` / `0X` prefix, no `.` or `p`).
    ///   Wraps modularly to `i64` for any number of hex digits.
    /// * Decimal floats with optional exponent.
    /// * Hex floats (e.g. `0x1.8p4`, `0xA.Bp3`) via
    ///   [`parse_hex_float`].
    ///
    /// Returns `None` on any other shape.  Used for the implicit
    /// string-to-number coercion that arithmetic operators perform
    /// on string operands (Lua 5.4 §3.4.3).
    ///
    /// [`parse_hex_float`]: Self::parse_hex_float
    pub fn parse_lua_str(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        // Strip an optional sign for the hex-integer probe; keep the
        // signed string for the decimal paths so they handle their
        // own signs uniformly.
        let (sign, body) = match s.as_bytes()[0] {
            b'-' => (-1i64, &s[1..]),
            b'+' => (1, &s[1..]),
            _ => (1, s),
        };
        if let Some(hex) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
            if hex.is_empty() {
                return None;
            }
            // Pure hex integer (no fractional part, no binary
            // exponent) wraps modularly per Lua 5.4 §3.1, so even
            // 26-digit literals like `0x13121110090807060504030201`
            // produce an i64 (the low 64 bits in two's complement).
            if !hex.contains('.') && !hex.contains('p') && !hex.contains('P') {
                if let Some(n) = Number::parse_hex_integer_wrapping(hex) {
                    return Some(Number::Integer(n.wrapping_mul(sign)));
                }
            }
            // Hex float (binary exponent or fractional part) —
            // delegate to the dedicated parser, which always
            // produces a float.
            return Number::parse_hex_float(s).map(Number::Float);
        }
        if let Ok(n) = s.parse::<i64>() {
            return Some(Number::Integer(n));
        }
        if let Ok(f) = s.parse::<f64>() {
            return Some(Number::Float(f));
        }
        None
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

    fn from_lua_ref(v: &Value) -> Result<Self, VmError> {
        match v {
            Value::Integer(n) => Ok(Number::Integer(*n)),
            Value::Float(f) => Ok(Number::Float(*f)),
            other => Err(VmError::BadArgument {
                position: 0,
                function: String::new(),
                expected: "number".to_owned(),
                got: other.type_name().to_owned(),
            }),
        }
    }
}

impl Number {
    /// Parse a Lua hex number literal as f64.  Handles:
    ///   - Hex floats: `0xA.Bp3`, `0x0.41`, `0xF0.0`
    ///   - Oversized hex integers that overflow i64
    pub fn parse_hex_float(s: &str) -> Option<f64> {
        let s = s.trim();
        let (negative, s) = if let Some(rest) = s.strip_prefix('-') {
            (true, rest)
        } else if let Some(rest) = s.strip_prefix('+') {
            (false, rest)
        } else {
            (false, s)
        };
        let hex = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X"))?;

        // Split off the optional binary exponent (p/P)
        let (mantissa_str, exp) = if let Some(pos) = hex.find(['p', 'P']) {
            let exp_str = &hex[pos + 1..];
            let exp: i32 = exp_str.parse().ok()?;
            (&hex[..pos], exp)
        } else {
            (hex, 0)
        };

        // Split mantissa into integer and fractional hex digit parts
        let (int_part, frac_part) = match mantissa_str.split_once('.') {
            Some((i, f)) => (i, f),
            None => (mantissa_str, ""),
        };

        // Parse integer part
        let mut value: f64 = if int_part.is_empty() {
            0.0
        } else {
            u64::from_str_radix(int_part, 16)
                .map(|v| v as f64)
                .unwrap_or_else(|_| {
                    // Very large integer part: parse digit by digit
                    let mut acc = 0.0_f64;
                    for ch in int_part.chars() {
                        let digit = ch.to_digit(16).unwrap_or(0) as f64;
                        acc = acc * 16.0 + digit;
                    }
                    acc
                })
        };

        // Parse fractional part
        if !frac_part.is_empty() {
            let mut place = 1.0 / 16.0;
            for ch in frac_part.chars() {
                let digit = ch.to_digit(16)? as f64;
                value += digit * place;
                place /= 16.0;
            }
        }

        // Apply binary exponent
        if exp != 0 {
            value *= (exp as f64).exp2();
        }

        if negative {
            value = -value;
        }

        Some(value)
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

    fn from_lua_ref(v: &Value) -> Result<Self, VmError> {
        let n = i64::from_lua_ref(v)?;
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
        LuaType::Number
    }
    fn value_type() -> Option<ValueType> {
        Some(ValueType::Number)
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

    fn from_lua_ref(v: &Value) -> Result<Self, VmError> {
        let n = i64::from_lua_ref(v)?;
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
        LuaType::Number
    }
    fn value_type() -> Option<ValueType> {
        Some(ValueType::Number)
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

    fn from_lua_ref(v: &Value) -> Result<Self, VmError> {
        let n = i64::from_lua_ref(v)?;
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
        LuaType::Number
    }
    fn value_type() -> Option<ValueType> {
        Some(ValueType::Number)
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

    fn from_lua_ref(v: &Value) -> Result<Self, VmError> {
        match v {
            Value::Float(f) => Ok(*f),
            Value::Integer(n) => Ok(*n as f64),
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
        LuaType::Number
    }
    fn value_type() -> Option<ValueType> {
        Some(ValueType::Number)
    }
}

impl FromLua for f32 {
    fn from_lua(v: Value) -> Result<Self, VmError> {
        Ok(f64::from_lua(v)? as f32)
    }

    fn from_lua_ref(v: &Value) -> Result<Self, VmError> {
        Ok(f64::from_lua_ref(v)? as f32)
    }
}

impl IntoLua for f32 {
    fn into_lua(self) -> Value {
        Value::Float(self as f64)
    }
}

impl LuaTyped for f32 {
    fn lua_type() -> LuaType {
        LuaType::Number
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
        Value::string(self.as_bytes())
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
    fn into_lua_multi(self) -> ValueVec {
        valuevec![]
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
            is_method: false,
            inferred_unannotated: false,
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
        LuaType::named("userdata")
    }

    fn value_type() -> Option<ValueType> {
        Some(ValueType::Userdata)
    }
}

// ---------------------------------------------------------------------------
// Ud<T> — typed userdata wrapper
// ---------------------------------------------------------------------------

/// A newtype around `Arc<T>` for concrete `Userdata` types, enabling
/// `FromLua` / `IntoLua` / `LuaTyped` via blanket impls.
///
/// Use this as a function parameter type to accept a specific userdata:
/// ```ignore
/// fn close(file: Option<Ud<LuaFile>>) -> Result<(), VmError> { ... }
/// ```
///
/// Dereferences to `Arc<T>` for ergonomic access.
#[derive(Debug, Clone)]
pub struct Ud<T: Userdata>(pub Arc<T>);

impl<T: Userdata> std::ops::Deref for Ud<T> {
    type Target = Arc<T>;
    fn deref(&self) -> &Arc<T> {
        &self.0
    }
}

impl<T: Userdata> From<Arc<T>> for Ud<T> {
    fn from(arc: Arc<T>) -> Self {
        Ud(arc)
    }
}

impl<T: Userdata> From<Ud<T>> for Arc<T> {
    fn from(ud: Ud<T>) -> Self {
        ud.0
    }
}

impl<T: Userdata> From<Ud<T>> for Value {
    fn from(ud: Ud<T>) -> Self {
        Value::Userdata(ud.0)
    }
}

impl<T: Userdata> From<Arc<T>> for Value {
    fn from(value: Arc<T>) -> Self {
        Value::Userdata(value)
    }
}

impl<T: Userdata + LuaTyped + 'static> FromLua for Ud<T> {
    fn from_lua(v: Value) -> Result<Self, VmError> {
        let expected = T::lua_type().to_string();
        match v {
            Value::Userdata(ud) => {
                // Erase explicit Send+Sync to get `dyn Userdata`,
                // which is where downcast_rs generates `downcast_arc`.
                let got = ud.type_name().to_owned();
                let ud: Arc<dyn Userdata> = ud;
                ud.downcast_arc::<T>()
                    .map(Ud)
                    .map_err(|_| VmError::BadArgument {
                        position: 0,
                        function: String::new(),
                        expected,
                        got,
                    })
            }
            other => Err(VmError::BadArgument {
                position: 0,
                function: String::new(),
                expected,
                got: other.type_name().to_owned(),
            }),
        }
    }
}

impl<T: Userdata> IntoLua for Ud<T> {
    fn into_lua(self) -> Value {
        Value::Userdata(self.0)
    }
}

impl<T: Userdata + LuaTyped> LuaTyped for Ud<T> {
    fn lua_type() -> LuaType {
        T::lua_type()
    }
}

// ---------------------------------------------------------------------------
// FromLuaBorrow impls
// ---------------------------------------------------------------------------

impl<'a> FromLuaBorrow<'a> for &'a Arc<dyn Userdata + Send + Sync> {
    fn from_lua_borrow(v: &'a Value) -> Result<Self, VmError> {
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

impl<'a, T: Userdata + LuaTyped + 'static> FromLuaBorrow<'a> for &'a T {
    fn from_lua_borrow(v: &'a Value) -> Result<Self, VmError> {
        let expected = T::lua_type().to_string();
        match v {
            Value::Userdata(u) => {
                let got = u.type_name().to_owned();
                let dyn_ref: &dyn Userdata = &**u;
                dyn_ref.downcast_ref::<T>().ok_or(VmError::BadArgument {
                    position: 0,
                    function: String::new(),
                    expected,
                    got,
                })
            }
            other => Err(VmError::BadArgument {
                position: 0,
                function: String::new(),
                expected,
                got: other.type_name().to_owned(),
            }),
        }
    }
}

impl<'a> FromLuaBorrow<'a> for &'a Value {
    fn from_lua_borrow(v: &'a Value) -> Result<Self, VmError> {
        Ok(v)
    }
}

impl<'a, T: FromLuaBorrow<'a>> FromLuaBorrow<'a> for Option<T> {
    fn from_lua_borrow(v: &'a Value) -> Result<Self, VmError> {
        match v {
            Value::Nil => Ok(None),
            other => T::from_lua_borrow(other).map(Some),
        }
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

    fn from_lua_ref(v: &Value) -> Result<Self, VmError> {
        match v {
            Value::Nil => Ok(None),
            other => T::from_lua_ref(other).map(Some),
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
// ValueVec as a single Lua value (an array table).
//
// `ValueVec` is the canonical "function return list" shape; its
// natural multi-return semantics live in [`IntoLuaMulti`] /
// [`FromLuaMulti`].  These impls let it *also* round-trip as a
// single Lua value when packed into a `Vec<ValueVec>` or similar
// position where each list is one Lua slot (an array table).
// ---------------------------------------------------------------------------

impl IntoLua for ValueVec {
    fn into_lua(self) -> Value {
        let table = Table::new();
        for (i, v) in self.into_iter().enumerate() {
            let _ = table.raw_set(Value::Integer((i + 1) as i64), v);
        }
        Value::Table(table)
    }
}

impl LuaTyped for ValueVec {
    fn lua_type() -> LuaType {
        LuaType::Table(Box::new(crate::types::TableLuaType {
            fields: vec![],
            indexer: Some((Box::new(LuaType::Integer), Box::new(LuaType::Any))),
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
    fn into_lua_multi(self) -> ValueVec {
        self.0
    }
}

/// `Variadic` collects the entire return list unchanged.
impl FromLuaMulti for Variadic {
    fn from_lua_multi(values: ValueVec) -> Result<Self, VmError> {
        Ok(Variadic(values))
    }
}

impl LuaTyped for Variadic {
    fn lua_type() -> LuaType {
        LuaType::Variadic(Box::new(LuaType::Any))
    }
}

// ---------------------------------------------------------------------------
// TypedVariadic<T> — homogeneously-typed variadic args/returns
// ---------------------------------------------------------------------------

/// A variadic argument or return list where every element has the same type.
///
/// Like [`Variadic`], but carries a concrete element type `T` instead of
/// erasing everything to `Value`.  This produces better `LuaType` metadata
/// (e.g. `...integer` instead of `...any`).
#[derive(Debug, Clone, Default)]
pub struct TypedVariadic<T>(pub Vec<T>);

impl<T: IntoLua> IntoLuaMulti for TypedVariadic<T> {
    fn into_lua_multi(self) -> ValueVec {
        self.0.into_iter().map(IntoLua::into_lua).collect()
    }
}

impl<T: FromLua> FromLuaMulti for TypedVariadic<T> {
    fn from_lua_multi(values: ValueVec) -> Result<Self, VmError> {
        // Tag each per-element error with its 1-based argument
        // position so users see `bad argument #3 to 'band' (...)`
        // instead of the placeholder position `0` produced by the
        // generic `T::from_lua` impl.
        values
            .into_iter()
            .enumerate()
            .map(|(i, v)| T::from_lua(v).map_err(|e| e.with_arg_position(i + 1)))
            .collect::<Result<Vec<_>, _>>()
            .map(TypedVariadic)
    }
}

impl<T: LuaTyped> LuaTyped for TypedVariadic<T> {
    fn lua_type() -> LuaType {
        LuaType::Variadic(Box::new(T::lua_type()))
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
    fn into_lua_multi(self) -> ValueVec {
        match self {
            StdlibResult::Ok(v) => v.into_lua_multi(),
            StdlibResult::Err(msg) => valuevec![Value::Nil, Value::string(msg)],
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
            fn into_lua_multi(self) -> ValueVec {
                let ($($name,)*) = self;
                valuevec![$($name.into_lua(),)*]
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
            fn from_lua_multi(values: ValueVec) -> Result<Self, VmError> {
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

#[cfg(test)]
mod parse_lua_str_tests {
    use super::Number;

    #[test]
    fn decimal_integer() {
        k9::assert_equal!(Number::parse_lua_str("42"), Some(Number::Integer(42)));
        k9::assert_equal!(Number::parse_lua_str("-7"), Some(Number::Integer(-7)));
        k9::assert_equal!(Number::parse_lua_str("+9"), Some(Number::Integer(9)));
    }

    #[test]
    fn decimal_float() {
        k9::assert_equal!(Number::parse_lua_str("3.14"), Some(Number::Float(3.14)));
        k9::assert_equal!(Number::parse_lua_str("1e2"), Some(Number::Float(100.0)));
        k9::assert_equal!(Number::parse_lua_str("-2.5e-1"), Some(Number::Float(-0.25)));
    }

    #[test]
    fn hex_integer_fits_i64() {
        k9::assert_equal!(Number::parse_lua_str("0xff"), Some(Number::Integer(0xff)));
        k9::assert_equal!(Number::parse_lua_str("-0xFF"), Some(Number::Integer(-0xff)));
    }

    #[test]
    fn hex_float_with_binary_exponent() {
        // 0x1.8p4 = 1.5 * 2^4 = 24.0
        k9::assert_equal!(Number::parse_lua_str("0x1.8p4"), Some(Number::Float(24.0)));
        // 0xA.Bp3 = 10.6875 * 2^3 = 85.5
        k9::assert_equal!(Number::parse_lua_str("0xA.Bp3"), Some(Number::Float(85.5)));
    }

    #[test]
    fn hex_float_no_exponent() {
        // 0xF0.0 = 240.0 (fractional part with no `p`).
        k9::assert_equal!(Number::parse_lua_str("0xF0.0"), Some(Number::Float(240.0)));
    }

    #[test]
    fn oversized_hex_integer_wraps_to_i64() {
        // Per Lua 5.4 §3.1, a hex integer literal of any length wraps
        // modularly: the low 64 bits become the signed integer.
        // 0x1000000000000000F (17 digits) keeps its bottom 16 hex
        // digits, which are `000000000000000F` = 15.
        k9::assert_equal!(
            Number::parse_lua_str("0x1000000000000000F"),
            Some(Number::Integer(0xF))
        );
        // Top-bit-set 16-digit literal becomes negative in two's
        // complement: 0xFFFFFFFFFFFFFFFF → -1.
        k9::assert_equal!(
            Number::parse_lua_str("0xFFFFFFFFFFFFFFFF"),
            Some(Number::Integer(-1))
        );
        // 26-digit Lua reference value: low 64 bits =
        // 0x0807060504030201 = 578437695752307201.
        k9::assert_equal!(
            Number::parse_lua_str("0x13121110090807060504030201"),
            Some(Number::Integer(0x0807060504030201))
        );
    }

    #[test]
    fn whitespace_is_trimmed() {
        k9::assert_equal!(Number::parse_lua_str("  10  "), Some(Number::Integer(10)));
        k9::assert_equal!(
            Number::parse_lua_str("\t0x1.8p4\n"),
            Some(Number::Float(24.0))
        );
    }

    #[test]
    fn rejects_non_numeric() {
        k9::assert_equal!(Number::parse_lua_str("hello"), None);
        k9::assert_equal!(Number::parse_lua_str(""), None);
        k9::assert_equal!(Number::parse_lua_str("   "), None);
        k9::assert_equal!(Number::parse_lua_str("0x"), None);
        k9::assert_equal!(Number::parse_lua_str("12abc"), None);
    }
}
