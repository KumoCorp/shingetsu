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
    fn min(args: crate::convert::Variadic) -> Result<Value, VmError> {
        let args = args.0;
        if args.is_empty() {
            return Err(VmError::BadArgument {
                position: 1,
                function: "min".to_owned(),
                expected: "number".to_owned(),
                got: "no value".to_owned(),
            });
        }
        let mut best = args[0].clone();
        let mut best_f = to_float(best.clone())?;
        for (i, v) in args.into_iter().enumerate().skip(1) {
            let f = to_float(v.clone()).map_err(|e| match e {
                VmError::BadArgument { expected, got, .. } => VmError::BadArgument {
                    position: i + 1,
                    function: "min".to_owned(),
                    expected,
                    got,
                },
                other => other,
            })?;
            if f < best_f {
                best = v;
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
    fn tointeger(x: Value) -> Value {
        match x {
            Value::Integer(_) => x,
            Value::Float(f) => {
                if f.fract() == 0.0 && f.is_finite() && f >= i64::MIN as f64 && f <= i64::MAX as f64
                {
                    Value::Integer(f as i64)
                } else {
                    Value::Nil
                }
            }
            _ => Value::Nil,
        }
    }

    /// `math.type(x)` — returns `"integer"` if x is an integer,
    /// `"float"` if x is a float, or `false` if x is not a number.
    #[function(rename = "type")]
    fn math_type(x: Value) -> Value {
        match x {
            Value::Integer(_) => Value::String(bytes::Bytes::from_static(b"integer")),
            Value::Float(_) => Value::String(bytes::Bytes::from_static(b"float")),
            _ => Value::Boolean(false),
        }
    }

    /// `math.max(x, ...)` — returns the maximum of its arguments.
    /// Compares using Lua `<` semantics (numbers only).
    #[function]
    fn max(args: crate::convert::Variadic) -> Result<Value, VmError> {
        let args = args.0;
        if args.is_empty() {
            return Err(VmError::BadArgument {
                position: 1,
                function: "max".to_owned(),
                expected: "number".to_owned(),
                got: "no value".to_owned(),
            });
        }
        let mut best = args[0].clone();
        let mut best_f = to_float(best.clone())?;
        for (i, v) in args.into_iter().enumerate().skip(1) {
            let f = to_float(v.clone()).map_err(|e| match e {
                VmError::BadArgument { expected, got, .. } => VmError::BadArgument {
                    position: i + 1,
                    function: "max".to_owned(),
                    expected,
                    got,
                },
                other => other,
            })?;
            if f > best_f {
                best = v;
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

/// `math.random([m [, n]])`
///
/// No args: returns a float in [0, 1).
/// One arg `m`: returns an integer in [1, m].
/// Two args `m, n`: returns an integer in [m, n].
fn math_random(rng: &Arc<Mutex<StdRng>>, args: Vec<Value>) -> Result<Vec<Value>, VmError> {
    let mut rng = rng.lock();
    match args.len() {
        0 => Ok(vec![Value::Float(rng.random_range(0.0..1.0))]),
        1 => {
            let m = to_int(args[0].clone(), 1, "random")?;
            if m < 1 {
                return Err(runtime_error(
                    "bad argument #1 to 'random' (interval is empty)".to_owned(),
                ));
            }
            Ok(vec![Value::Integer(rng.random_range(1..=m))])
        }
        _ => {
            let m = to_int(args[0].clone(), 1, "random")?;
            let n = to_int(args[1].clone(), 2, "random")?;
            if m > n {
                return Err(runtime_error(
                    "bad argument #2 to 'random' (interval is empty)".to_owned(),
                ));
            }
            Ok(vec![Value::Integer(rng.random_range(m..=n))])
        }
    }
}

/// `math.randomseed([x])`
///
/// Seeds the random number generator.  Without arguments, uses a
/// time-based seed.
fn math_randomseed(rng: &Arc<Mutex<StdRng>>, args: Vec<Value>) -> Result<Vec<Value>, VmError> {
    let seed = if args.is_empty() || args[0].is_nil() {
        // Use a time-based seed.
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    } else {
        to_int(args[0].clone(), 1, "randomseed")? as u64
    };
    *rng.lock() = StdRng::seed_from_u64(seed);
    Ok(vec![])
}

fn to_int(v: Value, pos: usize, func: &str) -> Result<i64, VmError> {
    match v {
        Value::Integer(n) => Ok(n),
        Value::Float(f) => Ok(f as i64),
        _ => Err(VmError::BadArgument {
            position: pos,
            function: func.to_owned(),
            expected: "number".to_owned(),
            got: v.type_name().to_owned(),
        }),
    }
}

fn runtime_error(msg: String) -> VmError {
    VmError::LuaError {
        display: msg.clone(),
        value: Value::String(bytes::Bytes::from(msg)),
    }
}

// =========================================================================
// Registration helpers
// =========================================================================

use std::sync::Arc;

use crate::wrap_native;

/// Build the math library table and register it as the `math` global.
pub fn register(env: &crate::GlobalEnv) -> Result<(), VmError> {
    let table = math_mod::build_module_table(env)?;

    // Per-environment RNG state for math.random / math.randomseed.
    let rng = Arc::new(Mutex::new(StdRng::seed_from_u64(0)));

    let rng_random = Arc::clone(&rng);
    table.raw_set(
        Value::String(bytes::Bytes::from_static(b"random")),
        wrap_native(b"random", move |args| math_random(&rng_random, args)),
    )?;

    let rng_seed = Arc::clone(&rng);
    table.raw_set(
        Value::String(bytes::Bytes::from_static(b"randomseed")),
        wrap_native(b"randomseed", move |args| math_randomseed(&rng_seed, args)),
    )?;

    env.set_global("math", Value::Table(table));
    Ok(())
}
