//! Lua `math` standard library.
//!
//! Registered as a global `math` table.

use crate::error::VmError;
use crate::value::Value;

/// Convert a `Value` to a `Number`, reporting the correct arg position.
fn value_to_number(v: &Value, position: usize, function: &str) -> Result<crate::Number, VmError> {
    match v {
        Value::Integer(n) => Ok(crate::Number::Integer(*n)),
        Value::Float(f) => Ok(crate::Number::Float(*f)),
        _ => Err(VmError::BadArgument {
            position,
            function: function.to_owned(),
            expected: "number".to_owned(),
            got: v.type_name().to_owned(),
        }),
    }
}

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

/// Return type for `math.type`: `"integer"`, `"float"`, or `false`.
enum MathTypeResult {
    Integer,
    Float,
    NotNumber,
}

impl crate::convert::IntoLua for MathTypeResult {
    fn into_lua(self) -> Value {
        match self {
            MathTypeResult::Integer => Value::string("integer"),
            MathTypeResult::Float => Value::string("float"),
            MathTypeResult::NotNumber => Value::Boolean(false),
        }
    }
}

impl crate::convert::LuaTyped for MathTypeResult {
    fn lua_type() -> crate::types::LuaType {
        use crate::types::LuaType;
        LuaType::Union(vec![LuaType::String, LuaType::Boolean])
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
    fn floor(x: Value) -> Result<crate::Number, VmError> {
        match x {
            Value::Integer(n) => Ok(crate::Number::Integer(n)),
            _ => {
                let f = to_float(x)?;
                let v = f.floor();
                if v >= i64::MIN as f64 && v <= i64::MAX as f64 && v.is_finite() {
                    Ok(crate::Number::Integer(v as i64))
                } else {
                    Ok(crate::Number::Float(v))
                }
            }
        }
    }

    /// `math.ceil(x)` — returns the smallest integer ≥ x.
    #[function]
    fn ceil(x: Value) -> Result<crate::Number, VmError> {
        match x {
            Value::Integer(n) => Ok(crate::Number::Integer(n)),
            _ => {
                let f = to_float(x)?;
                let v = f.ceil();
                if v >= i64::MIN as f64 && v <= i64::MAX as f64 && v.is_finite() {
                    Ok(crate::Number::Integer(v as i64))
                } else {
                    Ok(crate::Number::Float(v))
                }
            }
        }
    }

    /// `math.abs(x)` — returns the absolute value of x.
    #[function]
    fn abs(x: Value) -> Result<crate::Number, VmError> {
        match x {
            Value::Integer(n) => Ok(crate::Number::Integer(n.wrapping_abs())),
            Value::Float(f) => Ok(crate::Number::Float(f.abs())),
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
    fn modf(x: Value) -> Result<(crate::Number, f64), VmError> {
        let f = to_float(x)?;
        let trunc = f.trunc();
        let frac = f - trunc;
        let int_part = if trunc >= i64::MIN as f64 && trunc <= i64::MAX as f64 && trunc.is_finite()
        {
            crate::Number::Integer(trunc as i64)
        } else {
            crate::Number::Float(trunc)
        };
        Ok((int_part, frac))
    }

    // -----------------------------------------------------------------
    // Exponential & logarithmic
    // -----------------------------------------------------------------

    /// `math.sqrt(x)` — returns the square root of x.
    #[function]
    fn sqrt(x: Value) -> Result<f64, VmError> {
        Ok(to_float(x)?.sqrt())
    }

    /// `math.exp(x)` — returns e^x.
    #[function]
    fn exp(x: Value) -> Result<f64, VmError> {
        Ok(to_float(x)?.exp())
    }

    /// `math.log(x [, base])` — returns the logarithm of x.
    /// If `base` is given, returns `log(x) / log(base)` (i.e. log base b).
    /// Without `base`, returns the natural logarithm.
    #[function]
    fn log(x: Value, base: Option<Value>) -> Result<f64, VmError> {
        let x = to_float(x)?;
        match base {
            Some(b) => {
                let b = to_float(b)?;
                Ok(x.ln() / b.ln())
            }
            None => Ok(x.ln()),
        }
    }

    // -----------------------------------------------------------------
    // Trigonometric
    // -----------------------------------------------------------------

    /// `math.sin(x)` — sine of x (in radians).
    #[function]
    fn sin(x: Value) -> Result<f64, VmError> {
        Ok(to_float(x)?.sin())
    }

    /// `math.cos(x)` — cosine of x (in radians).
    #[function]
    fn cos(x: Value) -> Result<f64, VmError> {
        Ok(to_float(x)?.cos())
    }

    /// `math.tan(x)` — tangent of x (in radians).
    #[function]
    fn tan(x: Value) -> Result<f64, VmError> {
        Ok(to_float(x)?.tan())
    }

    /// `math.asin(x)` — arc sine (in radians).
    #[function]
    fn asin(x: Value) -> Result<f64, VmError> {
        Ok(to_float(x)?.asin())
    }

    /// `math.acos(x)` — arc cosine (in radians).
    #[function]
    fn acos(x: Value) -> Result<f64, VmError> {
        Ok(to_float(x)?.acos())
    }

    /// `math.atan(y [, x])` — arc tangent of y/x (in radians).
    /// With two arguments, uses `atan2(y, x)`.  With one, uses `atan(y)`.
    #[function]
    fn atan(y: Value, x: Option<Value>) -> Result<f64, VmError> {
        let y = to_float(y)?;
        match x {
            Some(xv) => Ok(y.atan2(to_float(xv)?)),
            None => Ok(y.atan()),
        }
    }

    // -----------------------------------------------------------------
    // Min / max
    // -----------------------------------------------------------------

    /// `math.min(x, ...)` — returns the minimum of its arguments.
    /// Compares using Lua `<` semantics (numbers only).
    #[function]
    fn min(args: crate::convert::Variadic) -> Result<crate::Number, VmError> {
        let args = args.0;
        if args.is_empty() {
            return Err(VmError::BadArgument {
                position: 1,
                function: "min".to_owned(),
                expected: "number".to_owned(),
                got: "no value".to_owned(),
            });
        }
        let mut best = value_to_number(&args[0], 1, "min")?;
        let mut best_f = best.into_float();
        for (i, v) in args.iter().enumerate().skip(1) {
            let n = value_to_number(v, i + 1, "min")?;
            let f = n.into_float();
            if f < best_f {
                best = n;
                best_f = f;
            }
        }
        Ok(best)
    }

    // -----------------------------------------------------------------
    // Integer operations
    // -----------------------------------------------------------------

    /// `math.tointeger(x)` — if x is convertible to an integer, returns
    /// that integer.  Otherwise returns `nil` (fail).
    #[function]
    fn tointeger(x: Value) -> Option<i64> {
        match x {
            Value::Integer(n) => Some(n),
            Value::Float(f) => {
                if f.fract() == 0.0 && f.is_finite() && f >= i64::MIN as f64 && f <= i64::MAX as f64
                {
                    Some(f as i64)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// `math.type(x)` — returns `"integer"` if x is an integer,
    /// `"float"` if x is a float, or `false` if x is not a number.
    #[function(rename = "type")]
    fn math_type(x: Value) -> MathTypeResult {
        match x {
            Value::Integer(_) => MathTypeResult::Integer,
            Value::Float(_) => MathTypeResult::Float,
            _ => MathTypeResult::NotNumber,
        }
    }

    /// `math.max(x, ...)` — returns the maximum of its arguments.
    /// Compares using Lua `<` semantics (numbers only).
    #[function]
    fn max(args: crate::convert::Variadic) -> Result<crate::Number, VmError> {
        let args = args.0;
        if args.is_empty() {
            return Err(VmError::BadArgument {
                position: 1,
                function: "max".to_owned(),
                expected: "number".to_owned(),
                got: "no value".to_owned(),
            });
        }
        let mut best = value_to_number(&args[0], 1, "max")?;
        let mut best_f = best.into_float();
        for (i, v) in args.iter().enumerate().skip(1) {
            let n = value_to_number(v, i + 1, "max")?;
            let f = n.into_float();
            if f > best_f {
                best = n;
                best_f = f;
            }
        }
        Ok(best)
    }
}

// =========================================================================
// Random number generator (uses `rand` crate)
// =========================================================================

use parking_lot::Mutex;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

fn runtime_error(msg: String) -> VmError {
    VmError::LuaError {
        display: msg.clone(),
        value: Value::string(msg),
    }
}

// =========================================================================
// Registration helpers
// =========================================================================

use std::sync::Arc;

use crate::function::Function;

/// Build the math library table and register it as the `math` global.
pub fn register(env: &crate::GlobalEnv) -> Result<(), VmError> {
    let table = math_mod::build_module_table(env)?;

    // Per-environment RNG state for math.random / math.randomseed.
    let rng = Arc::new(Mutex::new(StdRng::seed_from_u64(0)));

    let rng_random = Arc::clone(&rng);
    table.raw_set(
        Value::string("random"),
        Value::Function(Function::wrap(
            "random",
            move |m: Option<f64>, n: Option<f64>| {
                let mut rng = rng_random.lock();
                match (m.map(|v| v as i64), n.map(|v| v as i64)) {
                    (None, None) => Ok(Value::Float(rng.random_range(0.0..1.0))),
                    (Some(m), None) => {
                        if m < 1 {
                            return Err(runtime_error(
                                "bad argument #1 to 'random' (interval is empty)".to_owned(),
                            ));
                        }
                        Ok(Value::Integer(rng.random_range(1..=m)))
                    }
                    (Some(m), Some(n)) => {
                        if m > n {
                            return Err(runtime_error(
                                "bad argument #2 to 'random' (interval is empty)".to_owned(),
                            ));
                        }
                        Ok(Value::Integer(rng.random_range(m..=n)))
                    }
                    (None, Some(_)) => Err(runtime_error(
                        "bad argument #1 to 'random' (number expected, got nil)".to_owned(),
                    )),
                }
            },
        )),
    )?;

    let rng_seed = Arc::clone(&rng);
    table.raw_set(
        Value::string("randomseed"),
        Value::Function(Function::wrap("randomseed", move |x: Option<f64>| {
            let seed = match x {
                Some(n) => n as u64,
                None => std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0),
            };
            *rng_seed.lock() = StdRng::seed_from_u64(seed);
            Ok(())
        })),
    )?;

    env.set_global("math", Value::Table(table));
    Ok(())
}
