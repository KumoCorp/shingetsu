use std::sync::Arc;

use bytes::Bytes;

use crate::error::VmError;
use crate::function::Function;
use crate::table::Table;
use crate::userdata::Userdata;

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

    /// Coerce to a float for arithmetic, if possible.
    pub fn to_float(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            Value::Integer(i) => Some(*i as f64),
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
}

impl std::fmt::Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Nil => write!(f, "nil"),
            Value::Boolean(b) => write!(f, "{b}"),
            Value::Integer(i) => write!(f, "{i}"),
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
                // Lua prints floats with a trailing ".0" when they are whole.
                if fl.fract() == 0.0 && fl.is_finite() {
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

/// Arithmetic helpers used by the VM interpreter.
impl Value {
    pub fn arith_add(&self, rhs: &Value) -> Result<Value, VmError> {
        match (self, rhs) {
            (Value::Integer(a), Value::Integer(b)) => Ok(Value::Integer(a.wrapping_add(*b))),
            _ => match (self.to_float(), rhs.to_float()) {
                (Some(a), Some(b)) => Ok(Value::Float(a + b)),
                _ => Err(VmError::ArithmeticOnNonNumber {
                    type_name: if self.to_float().is_none() {
                        self.type_name()
                    } else {
                        rhs.type_name()
                    },
                }),
            },
        }
    }

    pub fn arith_sub(&self, rhs: &Value) -> Result<Value, VmError> {
        match (self, rhs) {
            (Value::Integer(a), Value::Integer(b)) => Ok(Value::Integer(a.wrapping_sub(*b))),
            _ => match (self.to_float(), rhs.to_float()) {
                (Some(a), Some(b)) => Ok(Value::Float(a - b)),
                _ => Err(VmError::ArithmeticOnNonNumber {
                    type_name: if self.to_float().is_none() {
                        self.type_name()
                    } else {
                        rhs.type_name()
                    },
                }),
            },
        }
    }

    pub fn arith_mul(&self, rhs: &Value) -> Result<Value, VmError> {
        match (self, rhs) {
            (Value::Integer(a), Value::Integer(b)) => Ok(Value::Integer(a.wrapping_mul(*b))),
            _ => match (self.to_float(), rhs.to_float()) {
                (Some(a), Some(b)) => Ok(Value::Float(a * b)),
                _ => Err(VmError::ArithmeticOnNonNumber {
                    type_name: if self.to_float().is_none() {
                        self.type_name()
                    } else {
                        rhs.type_name()
                    },
                }),
            },
        }
    }

    /// Float division — always returns float.
    pub fn arith_div(&self, rhs: &Value) -> Result<Value, VmError> {
        match (self.to_float(), rhs.to_float()) {
            (Some(a), Some(b)) => Ok(Value::Float(a / b)),
            _ => Err(VmError::ArithmeticOnNonNumber {
                type_name: if self.to_float().is_none() {
                    self.type_name()
                } else {
                    rhs.type_name()
                },
            }),
        }
    }

    /// Floor division.
    pub fn arith_idiv(&self, rhs: &Value) -> Result<Value, VmError> {
        match (self, rhs) {
            (Value::Integer(a), Value::Integer(b)) => {
                if *b == 0 {
                    return Err(VmError::ArithmeticOnNonNumber {
                        type_name: "zero (integer division by zero)",
                    });
                }
                Ok(Value::Integer(a.div_euclid(*b) * b.signum()))
            }
            _ => match (self.to_float(), rhs.to_float()) {
                (Some(a), Some(b)) => Ok(Value::Float((a / b).floor())),
                _ => Err(VmError::ArithmeticOnNonNumber {
                    type_name: if self.to_float().is_none() {
                        self.type_name()
                    } else {
                        rhs.type_name()
                    },
                }),
            },
        }
    }

    /// Modulo (Lua semantics: result has same sign as divisor).
    pub fn arith_mod(&self, rhs: &Value) -> Result<Value, VmError> {
        match (self, rhs) {
            (Value::Integer(a), Value::Integer(b)) => {
                if *b == 0 {
                    return Err(VmError::ArithmeticOnNonNumber {
                        type_name: "zero (integer modulo by zero)",
                    });
                }
                Ok(Value::Integer(a.rem_euclid(*b) * b.signum()))
            }
            _ => match (self.to_float(), rhs.to_float()) {
                (Some(a), Some(b)) => Ok(Value::Float(a - (a / b).floor() * b)),
                _ => Err(VmError::ArithmeticOnNonNumber {
                    type_name: if self.to_float().is_none() {
                        self.type_name()
                    } else {
                        rhs.type_name()
                    },
                }),
            },
        }
    }

    /// Exponentiation — always returns float.
    pub fn arith_pow(&self, rhs: &Value) -> Result<Value, VmError> {
        match (self.to_float(), rhs.to_float()) {
            (Some(a), Some(b)) => Ok(Value::Float(a.powf(b))),
            _ => Err(VmError::ArithmeticOnNonNumber {
                type_name: if self.to_float().is_none() {
                    self.type_name()
                } else {
                    rhs.type_name()
                },
            }),
        }
    }

    pub fn arith_neg(&self) -> Result<Value, VmError> {
        match self {
            Value::Integer(i) => Ok(Value::Integer(i.wrapping_neg())),
            Value::Float(f) => Ok(Value::Float(-f)),
            _ => Err(VmError::ArithmeticOnNonNumber {
                type_name: self.type_name(),
            }),
        }
    }

    /// Bitwise AND — integer only.
    pub fn arith_band(&self, rhs: &Value) -> Result<Value, VmError> {
        match (self.as_integer(), rhs.as_integer()) {
            (Some(a), Some(b)) => Ok(Value::Integer(a & b)),
            _ => Err(VmError::ArithmeticOnNonNumber {
                type_name: if self.as_integer().is_none() {
                    self.type_name()
                } else {
                    rhs.type_name()
                },
            }),
        }
    }

    pub fn arith_bor(&self, rhs: &Value) -> Result<Value, VmError> {
        match (self.as_integer(), rhs.as_integer()) {
            (Some(a), Some(b)) => Ok(Value::Integer(a | b)),
            _ => Err(VmError::ArithmeticOnNonNumber {
                type_name: if self.as_integer().is_none() {
                    self.type_name()
                } else {
                    rhs.type_name()
                },
            }),
        }
    }

    pub fn arith_bxor(&self, rhs: &Value) -> Result<Value, VmError> {
        match (self.as_integer(), rhs.as_integer()) {
            (Some(a), Some(b)) => Ok(Value::Integer(a ^ b)),
            _ => Err(VmError::ArithmeticOnNonNumber {
                type_name: if self.as_integer().is_none() {
                    self.type_name()
                } else {
                    rhs.type_name()
                },
            }),
        }
    }

    pub fn arith_bnot(&self) -> Result<Value, VmError> {
        match self.as_integer() {
            Some(i) => Ok(Value::Integer(!i)),
            None => Err(VmError::ArithmeticOnNonNumber {
                type_name: self.type_name(),
            }),
        }
    }

    pub fn arith_shl(&self, rhs: &Value) -> Result<Value, VmError> {
        match (self.as_integer(), rhs.as_integer()) {
            (Some(a), Some(b)) => {
                let shift = b.rem_euclid(64) as u32;
                Ok(Value::Integer(a << shift))
            }
            _ => Err(VmError::ArithmeticOnNonNumber {
                type_name: if self.as_integer().is_none() {
                    self.type_name()
                } else {
                    rhs.type_name()
                },
            }),
        }
    }

    pub fn arith_shr(&self, rhs: &Value) -> Result<Value, VmError> {
        match (self.as_integer(), rhs.as_integer()) {
            (Some(a), Some(b)) => {
                let shift = b.rem_euclid(64) as u32;
                Ok(Value::Integer(((a as u64) >> shift) as i64))
            }
            _ => Err(VmError::ArithmeticOnNonNumber {
                type_name: if self.as_integer().is_none() {
                    self.type_name()
                } else {
                    rhs.type_name()
                },
            }),
        }
    }
}
