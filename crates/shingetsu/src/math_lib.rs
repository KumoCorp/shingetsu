//! Lua `math` standard library.
//!
//! Registered as a global `math` table.

use crate::error::VmError;
use crate::value::Value;

/// Coerce a Lua value to f64 for math functions.
fn to_float(v: Value) -> Result<f64, VmError> {
    match v {
        Value::Float(f) => Ok(f),
        Value::Integer(n) => Ok(n as f64),
        _ => Err(VmError::BadArgument {
            position: 0, // patched by proc-macro
            function: String::new(),
            expected: "number".to_owned(),
            got: v.type_name().to_owned(),
        }),
    }
}

#[crate::module(name = "math")]
pub mod math_mod {
    use super::*;

    // -----------------------------------------------------------------
    // Constants
    // -----------------------------------------------------------------

    #[field]
    fn pi() -> f64 {
        std::f64::consts::PI
    }

    #[field]
    fn huge() -> f64 {
        f64::INFINITY
    }

    #[field]
    fn maxinteger() -> i64 {
        i64::MAX
    }

    #[field]
    fn mininteger() -> i64 {
        i64::MIN
    }

    // -----------------------------------------------------------------
    // Rounding & sign
    // -----------------------------------------------------------------

    /// `math.floor(x)` — returns the largest integer ≤ x.
    /// Returns an integer when the result fits, otherwise a float.
    #[function]
    fn floor(x: Value) -> Result<Value, VmError> {
        match x {
            Value::Integer(_) => Ok(x),
            _ => {
                let f = to_float(x)?;
                let v = f.floor();
                if v >= i64::MIN as f64 && v <= i64::MAX as f64 && v.is_finite() {
                    Ok(Value::Integer(v as i64))
                } else {
                    Ok(Value::Float(v))
                }
            }
        }
    }

    /// `math.ceil(x)` — returns the smallest integer ≥ x.
    #[function]
    fn ceil(x: Value) -> Result<Value, VmError> {
        match x {
            Value::Integer(_) => Ok(x),
            _ => {
                let f = to_float(x)?;
                let v = f.ceil();
                if v >= i64::MIN as f64 && v <= i64::MAX as f64 && v.is_finite() {
                    Ok(Value::Integer(v as i64))
                } else {
                    Ok(Value::Float(v))
                }
            }
        }
    }

    /// `math.abs(x)` — returns the absolute value of x.
    #[function]
    fn abs(x: Value) -> Result<Value, VmError> {
        match x {
            Value::Integer(n) => Ok(Value::Integer(n.wrapping_abs())),
            Value::Float(f) => Ok(Value::Float(f.abs())),
            _ => Err(VmError::BadArgument {
                position: 1,
                function: "abs".to_owned(),
                expected: "number".to_owned(),
                got: x.type_name().to_owned(),
            }),
        }
    }

    /// `math.modf(x)` — returns the integral part and fractional part of x.
    /// The integral part is returned as an integer when it fits.
    #[function]
    fn modf(x: Value) -> Result<crate::convert::Variadic, VmError> {
        let f = to_float(x)?;
        let trunc = f.trunc();
        let frac = f - trunc;
        let int_part = if trunc >= i64::MIN as f64 && trunc <= i64::MAX as f64 && trunc.is_finite()
        {
            Value::Integer(trunc as i64)
        } else {
            Value::Float(trunc)
        };
        Ok(crate::convert::Variadic(vec![int_part, Value::Float(frac)]))
    }
}

/// Build the math library table and register it as the `math` global.
pub fn register(env: &crate::GlobalEnv) -> Result<(), VmError> {
    let table = math_mod::build_module_table(env)?;
    env.set_global("math", Value::Table(table));
    Ok(())
}
