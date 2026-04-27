use std::sync::Arc;

use smallvec::SmallVec;

use crate::byte_string::Bytes;
use crate::convert::Number;
use crate::error::VmError;
use crate::function::Function;
use crate::table::Table;
use crate::userdata::Userdata;

/// A small-vec optimised container for multi-valued Lua results.
///
/// Most Lua calls return 0–3 values, so keeping up to 3 inline avoids
/// a heap allocation on every native call/return.
pub type ValueVec = SmallVec<[Value; 3]>;

/// Construct a [`ValueVec`] with the same syntax as `vec![]`.
#[macro_export]
macro_rules! valuevec {
    ($($args:tt)*) => {
        {
            let v: $crate::ValueVec = $crate::smallvec::smallvec![$($args)*];
            v
        }
    };
}

/// A Lua runtime value.
///
/// `Clone` is cheap for all variants:
/// - `Nil`, `Boolean`, `Integer`, `Float` — copy.
/// - `String` — `Bytes` clone is O(1).
/// - `Table`, `Function` — `Arc` clone.
/// - `Userdata` — `Arc` clone.
#[derive(Clone)]
pub enum Value {
    Nil,
    Boolean(bool),
    Integer(i64),
    Float(f64),
    String(Bytes),
    Table(Table),
    Function(Function),
    Userdata(Arc<dyn Userdata + Send + Sync>),
}

impl Value {
    /// Convenience constructor for `Value::String`.
    ///
    /// Accepts anything that implements `Into<Bytes>`: string literals
    /// (`"hello"`), `&[u8]` slices, owned `String` and `Vec<u8>`,
    /// and `Bytes` itself.  Short strings (≤15 bytes) are stored
    /// inline; longer strings use a refcounted heap allocation
    /// with O(1) clone.
    pub fn string(s: impl Into<Bytes>) -> Self {
        Value::String(s.into())
    }

    /// Returns the Lua type name string, as returned by `type()`.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Nil => "nil",
            Value::Boolean(_) => "boolean",
            Value::Integer(_) => "number",
            Value::Float(_) => "number",
            Value::String(_) => "string",
            Value::Table(_) => "table",
            Value::Function(_) => "function",
            Value::Userdata(_) => "userdata",
        }
    }

    /// `false` and `nil` are falsy; everything else is truthy.
    pub fn is_truthy(&self) -> bool {
        !matches!(self, Value::Nil | Value::Boolean(false))
    }

    pub fn is_nil(&self) -> bool {
        matches!(self, Value::Nil)
    }

    /// Returns `true` for value types that are plain data with no heap
    /// resources — safe to bitwise-copy without Clone and safe to
    /// overwrite without Drop.
    #[inline(always)]
    pub fn is_copy(&self) -> bool {
        matches!(
            self,
            Value::Nil | Value::Boolean(_) | Value::Integer(_) | Value::Float(_)
        )
    }

    /// Coerce to a float for arithmetic, if possible.  Honours the
    /// Lua 5.4 string-to-number rule — numeric-valued strings are
    /// parsed as floats.  Used by ops that always return a float
    /// (`/`, `^`) and as the fallback for ops where one operand is
    /// non-integer.
    pub fn to_float(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            Value::Integer(i) => Some(*i as f64),
            Value::String(s) => {
                Number::parse_lua_str(std::str::from_utf8(s).ok()?).map(Number::into_float)
            }
            _ => None,
        }
    }

    /// Coerce to an integer, if the value is already an integer.
    pub fn as_integer(&self) -> Option<i64> {
        match self {
            Value::Integer(i) => Some(*i),
            _ => None,
        }
    }

    /// Coerce to a numeric value for arithmetic, applying Lua 5.4's
    /// implicit string-to-number rule (§3.4.3).  An integer-valued
    /// string returns `Number::Integer`; a float-valued or scientific
    /// string returns `Number::Float`; anything else returns `None`.
    /// The distinction matters for arithmetic: `int + int → int` only
    /// when both sides coerce to `Integer`; otherwise the result is
    /// a float (Lua's “usual rule”).
    pub fn coerce_to_number(&self) -> Option<Number> {
        match self {
            Value::Integer(i) => Some(Number::Integer(*i)),
            Value::Float(f) => Some(Number::Float(*f)),
            Value::String(s) => Number::parse_lua_str(std::str::from_utf8(s).ok()?),
            _ => None,
        }
    }

    /// Convert to a string value using `tostring` semantics for types
    /// that don't require metamethod dispatch. Returns `None` for
    /// tables and userdata, which may have `__tostring` metamethods.
    pub fn to_string_value(&self) -> Option<Value> {
        match self {
            Value::String(_) => Some(self.clone()),
            Value::Integer(_) | Value::Float(_) => {
                Some(Value::String(Bytes::from(self.to_string())))
            }
            Value::Boolean(b) => Some(Value::String(Bytes::from(if *b {
                "true"
            } else {
                "false"
            }))),
            Value::Nil => Some(Value::String(Bytes::from("nil"))),
            Value::Table(_) | Value::Userdata(_) => None,
            Value::Function(_) => Some(Value::String(Bytes::from(self.to_string()))),
        }
    }
}

impl std::fmt::Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Nil => write!(f, "nil"),
            Value::Boolean(b) => write!(f, "{b}"),
            Value::Integer(i) => write!(f, "{i}"),
            Value::Float(fl) if fl.is_nan() => write!(f, "nan"),
            Value::Float(fl) => write!(f, "{fl}"),
            Value::String(s) => write!(f, "{:?}", s),
            Value::Table(t) => write!(f, "table: {:p}", Arc::as_ptr(&t.0)),
            Value::Function(_) => write!(f, "function"),
            Value::Userdata(u) => write!(f, "userdata: {}", u.type_name()),
        }
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Nil => write!(f, "nil"),
            Value::Boolean(b) => write!(f, "{b}"),
            Value::Integer(i) => write!(f, "{i}"),
            Value::Float(fl) => {
                if fl.is_nan() {
                    write!(f, "nan")
                } else if fl.fract() == 0.0 && fl.is_finite() {
                    // Lua prints floats with a trailing ".0" when they are whole.
                    write!(f, "{fl:.1}")
                } else {
                    write!(f, "{fl}")
                }
            }
            Value::String(s) => {
                write!(f, "{}", bstr::BStr::new(s.as_ref()))
            }
            Value::Table(t) => write!(f, "table: {:p}", Arc::as_ptr(&t.0)),
            Value::Function(_) => write!(f, "function"),
            Value::Userdata(u) => write!(f, "userdata: {}", u.type_name()),
        }
    }
}

impl Value {
    /// Returns a pointer-like identity for `%p` formatting.
    /// Types with heap identity (string, table, function, userdata) return
    /// their backing pointer; value types return null.
    pub fn to_pointer(&self) -> *const () {
        match self {
            Value::String(s) => s.as_ptr() as *const (),
            Value::Table(t) => Arc::as_ptr(&t.0) as *const (),
            Value::Function(f) => Arc::as_ptr(&f.0) as *const (),
            Value::Userdata(u) => Arc::as_ptr(u) as *const (),
            _ => std::ptr::null(),
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Nil, Value::Nil) => true,
            (Value::Boolean(a), Value::Boolean(b)) => a == b,
            (Value::Integer(a), Value::Integer(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            // Lua: integer == float when numerically equal.
            (Value::Integer(i), Value::Float(f)) => (*i as f64) == *f,
            (Value::Float(f), Value::Integer(i)) => *f == (*i as f64),
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Table(a), Value::Table(b)) => Arc::ptr_eq(&a.0, &b.0),
            (Value::Function(a), Value::Function(b)) => a.ptr_eq(b),
            (Value::Userdata(a), Value::Userdata(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

/// Apply an integer-or-float arithmetic op to two operands, honouring
/// Lua 5.4's coercion rules: both operands integer → integer result
/// (via `int_op`); otherwise both coerced via `to_float` and combined
/// with `float_op`.
fn arith_int_or_float(
    lhs: &Value,
    rhs: &Value,
    int_op: impl FnOnce(i64, i64) -> i64,
    float_op: impl FnOnce(f64, f64) -> f64,
) -> Result<Value, VmError> {
    match (lhs.coerce_to_number(), rhs.coerce_to_number()) {
        (Some(Number::Integer(a)), Some(Number::Integer(b))) => Ok(Value::Integer(int_op(a, b))),
        (Some(a), Some(b)) => Ok(Value::Float(float_op(a.into_float(), b.into_float()))),
        _ => Err(arithmetic_error(lhs, rhs)),
    }
}

/// Build an `ArithmeticOnNonNumber` error pointing at whichever
/// operand failed to coerce.
fn arithmetic_error(lhs: &Value, rhs: &Value) -> VmError {
    let bad = if lhs.coerce_to_number().is_none() {
        lhs
    } else {
        rhs
    };
    VmError::ArithmeticOnNonNumber {
        type_name: bad.type_name(),
        name: None,
    }
}

/// Arithmetic helpers used by the VM interpreter.
impl Value {
    pub fn arith_add(&self, rhs: &Value) -> Result<Value, VmError> {
        arith_int_or_float(self, rhs, i64::wrapping_add, |a, b| a + b)
    }

    pub fn arith_sub(&self, rhs: &Value) -> Result<Value, VmError> {
        arith_int_or_float(self, rhs, i64::wrapping_sub, |a, b| a - b)
    }

    pub fn arith_mul(&self, rhs: &Value) -> Result<Value, VmError> {
        arith_int_or_float(self, rhs, i64::wrapping_mul, |a, b| a * b)
    }

    /// Float division — always returns float.
    pub fn arith_div(&self, rhs: &Value) -> Result<Value, VmError> {
        match (self.to_float(), rhs.to_float()) {
            (Some(a), Some(b)) => Ok(Value::Float(a / b)),
            _ => Err(arithmetic_error(self, rhs)),
        }
    }

    /// Floor division.
    pub fn arith_idiv(&self, rhs: &Value) -> Result<Value, VmError> {
        match (self.coerce_to_number(), rhs.coerce_to_number()) {
            (Some(Number::Integer(a)), Some(Number::Integer(b))) => {
                if b == 0 {
                    return Err(VmError::ArithmeticOnNonNumber {
                        type_name: "zero (integer division by zero)",
                        name: None,
                    });
                }
                Ok(Value::Integer(a.div_euclid(b) * b.signum()))
            }
            (Some(a), Some(b)) => Ok(Value::Float((a.into_float() / b.into_float()).floor())),
            _ => Err(arithmetic_error(self, rhs)),
        }
    }

    /// Modulo (Lua semantics: result has same sign as divisor).
    pub fn arith_mod(&self, rhs: &Value) -> Result<Value, VmError> {
        match (self.coerce_to_number(), rhs.coerce_to_number()) {
            (Some(Number::Integer(a)), Some(Number::Integer(b))) => {
                if b == 0 {
                    return Err(VmError::ArithmeticOnNonNumber {
                        type_name: "zero (integer modulo by zero)",
                        name: None,
                    });
                }
                Ok(Value::Integer(a.rem_euclid(b) * b.signum()))
            }
            (Some(a), Some(b)) => {
                let af = a.into_float();
                let bf = b.into_float();
                Ok(Value::Float(af - (af / bf).floor() * bf))
            }
            _ => Err(arithmetic_error(self, rhs)),
        }
    }

    /// Exponentiation — always returns float.
    pub fn arith_pow(&self, rhs: &Value) -> Result<Value, VmError> {
        match (self.to_float(), rhs.to_float()) {
            (Some(a), Some(b)) => Ok(Value::Float(a.powf(b))),
            _ => Err(arithmetic_error(self, rhs)),
        }
    }

    pub fn arith_neg(&self) -> Result<Value, VmError> {
        match self.coerce_to_number() {
            Some(Number::Integer(i)) => Ok(Value::Integer(i.wrapping_neg())),
            Some(Number::Float(f)) => Ok(Value::Float(-f)),
            None => Err(VmError::ArithmeticOnNonNumber {
                type_name: self.type_name(),
                name: None,
            }),
        }
    }

    /// Bitwise AND — integer only.
    pub fn arith_band(&self, rhs: &Value) -> Result<Value, VmError> {
        bitwise_int_op(self, rhs, |a, b| a & b)
    }

    pub fn arith_bor(&self, rhs: &Value) -> Result<Value, VmError> {
        bitwise_int_op(self, rhs, |a, b| a | b)
    }

    pub fn arith_bxor(&self, rhs: &Value) -> Result<Value, VmError> {
        bitwise_int_op(self, rhs, |a, b| a ^ b)
    }

    pub fn arith_bnot(&self) -> Result<Value, VmError> {
        match self.coerce_to_integer() {
            Some(i) => Ok(Value::Integer(!i)),
            None => Err(VmError::ArithmeticOnNonNumber {
                type_name: self.type_name(),
                name: None,
            }),
        }
    }

    pub fn arith_shl(&self, rhs: &Value) -> Result<Value, VmError> {
        bitwise_int_op(self, rhs, |a, b| lua_shift_left(a, b))
    }

    pub fn arith_shr(&self, rhs: &Value) -> Result<Value, VmError> {
        // Right-shift by `b` is left-shift by `-b`; the helper handles
        // both directions and the |b| >= 64 “all bits shifted out” rule.
        bitwise_int_op(self, rhs, |a, b| lua_shift_left(a, b.wrapping_neg()))
    }

    /// Coerce to an integer for bitwise operations.  Per Lua 5.4
    /// §3.4.3, bitwise operands accept integers and floats with an
    /// integer value (e.g. `2.0`).  Strings are *not* coerced —
    /// matching Lua's `attempt to perform bitwise operation on a
    /// string value` error.
    pub fn coerce_to_integer(&self) -> Option<i64> {
        match self {
            Value::Integer(i) => Some(*i),
            Value::Float(f) => {
                if f.fract() == 0.0 && *f >= i64::MIN as f64 && *f <= i64::MAX as f64 {
                    Some(*f as i64)
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

/// Lua 5.4 §3.4.3 shift semantics.  Positive `n` is a left shift,
/// negative `n` is a right shift.  Shifts whose absolute value is
/// at least the integer width (64) result in zero — "all bits are
/// shifted out".  Right shifts are *logical* (zero-fill), so the
/// inversion via `wrapping_neg` operates on the bit pattern, not on
/// arithmetic.
fn lua_shift_left(a: i64, n: i64) -> i64 {
    if n >= 64 || n <= -64 {
        return 0;
    }
    if n >= 0 {
        ((a as u64) << n as u32) as i64
    } else {
        ((a as u64) >> (-n) as u32) as i64
    }
}

/// Apply a binary bitwise op, coercing each operand via
/// [`Value::coerce_to_integer`].
fn bitwise_int_op(
    lhs: &Value,
    rhs: &Value,
    op: impl FnOnce(i64, i64) -> i64,
) -> Result<Value, VmError> {
    match (lhs.coerce_to_integer(), rhs.coerce_to_integer()) {
        (Some(a), Some(b)) => Ok(Value::Integer(op(a, b))),
        _ => {
            let bad = if lhs.coerce_to_integer().is_none() {
                lhs
            } else {
                rhs
            };
            Err(VmError::ArithmeticOnNonNumber {
                type_name: bad.type_name(),
                name: None,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_size() {
        k9::assert_equal!(std::mem::size_of::<Value>(), 32);
    }
}
