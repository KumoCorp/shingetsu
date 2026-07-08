//! Implementation of the `math` standard library module.

use crate::value::Value;
use crate::VmError;

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

/// Mathematical functions and numeric constants.
///
/// Most functions accept either an integer or a float argument and
/// promote to floats internally.  The few that preserve the integer
/// subtype — `math.floor`, `math.ceil`, `math.abs`, `math.modf`,
/// `math.min`, `math.max`, `math.random` — say so in their
/// individual documentation.
///
/// Trigonometric functions take and return angles in radians;
/// use `math.deg` and `math.rad` to convert between degrees and
/// radians.  Random-number functions use a per-environment RNG so
/// concurrent VMs don't share state; reseed with
/// `math.randomseed` for reproducible streams.
#[crate::module(name = "math")]
pub mod math_mod {
    use super::*;

    /// The mathematical constant π as a float.
    ///
    /// # Examples
    ///
    /// ```lua
    /// print(math.pi)              --> 3.1415926535898
    /// ```
    #[field]
    fn pi() -> f64 {
        std::f64::consts::PI
    }

    /// Not-a-number (NaN) as a float.
    ///
    /// NaN is the only value that is not equal to itself: `math.nan ~= math.nan`
    /// is `true`.  Use `math.isnan` to test for NaN rather than direct
    /// comparison.
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.isnan(math.nan))
    /// ```
    #[field]
    fn nan() -> f64 {
        f64::NAN
    }

    /// Euler's number *e* as a float.
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.log(math.e) == 1.0)
    /// ```
    #[field]
    fn e() -> f64 {
        std::f64::consts::E
    }

    /// The golden ratio φ as a float.
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.phi > 1.61)
    /// assert(math.phi < 1.62)
    /// ```
    #[field]
    fn phi() -> f64 {
        1.618_033_988_749_895
    }

    /// The square root of 2 as a float.
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.sqrt2 > 1.41)
    /// assert(math.sqrt2 < 1.42)
    /// ```
    #[field]
    fn sqrt2() -> f64 {
        std::f64::consts::SQRT_2
    }

    /// The circle constant τ (2π) as a float.
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.tau == 2 * math.pi)
    /// ```
    #[field]
    fn tau() -> f64 {
        std::f64::consts::TAU
    }

    /// Positive infinity as a float.
    ///
    /// Useful as a starting value when finding a minimum, or as a
    /// sentinel for "no upper bound".
    ///
    /// # Examples
    ///
    /// ```lua
    /// -- Use as the starting value when finding a minimum.
    /// local smallest = math.huge
    /// for _, v in ipairs({3, 1, 4, 1, 5}) do
    ///     if v < smallest then smallest = v end
    /// end
    /// assert(smallest == 1)
    /// ```
    #[field]
    fn huge() -> f64 {
        f64::INFINITY
    }

    /// The largest representable integer (`2^63 - 1`).
    ///
    /// Adding `1` to `math.maxinteger` wraps around to
    /// `math.mininteger`, matching Lua 5.4's two's-complement
    /// integer semantics.
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.maxinteger == 9223372036854775807)
    /// assert(math.maxinteger + 1 == math.mininteger)
    /// ```
    #[field]
    fn maxinteger() -> i64 {
        i64::MAX
    }

    /// The smallest representable integer (`-2^63`).
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.mininteger == -9223372036854775808)
    /// assert(math.mininteger - 1 == math.maxinteger)
    /// ```
    #[field]
    fn mininteger() -> i64 {
        i64::MIN
    }

    // -----------------------------------------------------------------
    // Rounding & sign
    // -----------------------------------------------------------------

    /// Round `x` down to the nearest integer.
    ///
    /// Returns the largest integer that is less than or equal to
    /// `x`.  When `x` is already an integer it is returned
    /// unchanged.  When the floor of `x` fits in a Lua integer the
    /// result is an integer; otherwise (very large floats) the
    /// result stays a float.
    ///
    /// # Parameters
    ///
    /// - `x` — the number to floor
    ///
    /// # Returns
    ///
    /// - the floor of `x`
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.floor(2.7) == 2)
    /// assert(math.floor(-2.3) == -3)
    /// assert(math.floor(5) == 5)
    /// ```
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

    /// Round `x` up to the nearest integer.
    ///
    /// Returns the smallest integer that is greater than or equal
    /// to `x`.  When `x` is already an integer it is returned
    /// unchanged.
    ///
    /// # Parameters
    ///
    /// - `x` — the number to ceil
    ///
    /// # Returns
    ///
    /// - the ceiling of `x`
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.ceil(2.3) == 3)
    /// assert(math.ceil(-2.7) == -2)
    /// assert(math.ceil(5) == 5)
    /// ```
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

    /// Return the absolute value of `x`.
    ///
    /// Preserves the integer-vs-float subtype: an integer input
    /// returns an integer, a float input returns a float.
    ///
    /// Note that `math.abs(math.mininteger)` overflows and wraps
    /// back to `math.mininteger`, since the positive value cannot
    /// be represented as a Lua integer.
    ///
    /// # Parameters
    ///
    /// - `x` — the number to take the absolute value of
    ///
    /// # Returns
    ///
    /// - the absolute value of `x`
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.abs(-5) == 5)
    /// assert(math.abs(3.14) == 3.14)
    /// assert(math.abs(0) == 0)
    /// ```
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

    /// Split `x` into its integral and fractional parts.
    ///
    /// Returns two values: the integral part (with the same sign
    /// as `x`, returned as an integer when it fits) and the
    /// fractional part (always a float).
    ///
    /// # Parameters
    ///
    /// - `x` — the number to split
    ///
    /// # Returns
    ///
    /// - the integral part of `x`
    /// - the fractional part of `x`
    ///
    /// # Examples
    ///
    /// ```lua
    /// local int, frac = math.modf(3.75)
    /// assert(int == 3)
    /// assert(frac == 0.75)
    /// ```
    ///
    /// ```lua
    /// local int, frac = math.modf(-2.25)
    /// assert(int == -2)
    /// assert(frac == -0.25)
    /// ```
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

    /// Return the square root of `x`.
    ///
    /// Returns the special floating-point value `NaN` ("not a
    /// number") when `x` is negative.
    ///
    /// # Parameters
    ///
    /// - `x` — a non-negative number
    ///
    /// # Returns
    ///
    /// - the square root of `x`, as a float
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.sqrt(16) == 4.0)
    /// assert(math.sqrt(0) == 0.0)
    /// print(math.sqrt(2))             --> 1.4142135623731
    /// ```
    #[function]
    fn sqrt(x: Value) -> Result<f64, VmError> {
        Ok(to_float(x)?.sqrt())
    }

    /// Return `e` raised to the power of `x`.
    ///
    /// `e` is Euler's number, approximately `2.718281828`.  This is
    /// the inverse of `math.log` (the natural logarithm).
    ///
    /// # Parameters
    ///
    /// - `x` — the exponent
    ///
    /// # Returns
    ///
    /// - `e^x`, as a float
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.exp(0) == 1.0)
    /// print(math.exp(1))              --> 2.718281828459 (Euler's number)
    /// ```
    #[function]
    fn exp(x: Value) -> Result<f64, VmError> {
        Ok(to_float(x)?.exp())
    }

    /// Return the logarithm of `x`.
    ///
    /// With one argument, returns the natural logarithm (base `e`).
    /// With two arguments, returns `log(x)` divided by `log(base)`,
    /// which gives the logarithm of `x` in the given base.
    ///
    /// # Parameters
    ///
    /// - `x` — the value to take the logarithm of
    /// - `base` — logarithm base; defaults to `e` (natural log)
    ///
    /// # Returns
    ///
    /// - the logarithm, as a float
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.log(1) == 0.0)
    /// print(math.log(math.exp(1)))    --> 1.0 (natural log of e)
    /// print(math.log(1000, 10))       --> 3.0 (log base 10)
    /// ```
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

    /// Return the base-10 logarithm of `x`.
    ///
    /// Equivalent to `math.log(x, 10)`, but more readable and
    /// slightly faster.  Provided for compatibility with Lua 5.1
    /// and Luau.
    ///
    /// # Parameters
    ///
    /// - `x` — the value to take the logarithm of
    ///
    /// # Returns
    ///
    /// - the base-10 logarithm, as a float
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.log10(1000) == 3.0)
    /// assert(math.log10(100) == 2.0)
    /// ```
    #[function(rename = "log10")]
    fn log10_compat(x: Value) -> Result<f64, VmError> {
        Ok(to_float(x)?.log10())
    }

    /// Return `x` raised to the power of `y`.
    ///
    /// Equivalent to the `^` operator.  Provided for compatibility
    /// with Lua 5.1 and Luau.
    ///
    /// # Parameters
    ///
    /// - `x` — the base
    /// - `y` — the exponent
    ///
    /// # Returns
    ///
    /// - `x^y`, as a float
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.pow(2, 10) == 1024.0)
    /// assert(math.pow(100, 0.5) == 10.0)
    /// ```
    #[function]
    fn pow(x: Value, y: Value) -> Result<f64, VmError> {
        Ok(to_float(x)?.powf(to_float(y)?))
    }

    /// Split `x` into a mantissa and exponent.
    ///
    /// Returns two values: a mantissa `m` in the range
    /// `[0.5, 1)` (or zero) and an integer exponent `e` such that
    /// `x = m × 2^e`.
    ///
    /// # Parameters
    ///
    /// - `x` — the number to decompose
    ///
    /// # Returns
    ///
    /// - `m` — the mantissa, in `[0.5, 1)` or zero
    /// - `e` — the exponent, as an integer
    ///
    /// # Examples
    ///
    /// ```lua
    /// local m, e = math.frexp(8)
    /// assert(m == 0.5)
    /// assert(e == 4)
    /// local m2, e2 = math.frexp(3.14)
    /// assert(m2 * 2^e2 == 3.14)
    /// -- Zero, NaN, and infinity exit early with exponent 0.
    /// local mz, ez = math.frexp(0)
    /// assert(mz == 0)
    /// assert(ez == 0)
    /// ```
    #[function]
    fn frexp(x: Value) -> Result<(f64, i64), VmError> {
        let v = to_float(x)?;
        if v == 0.0 || v.is_nan() || v.is_infinite() {
            return Ok((v, 0));
        }
        let bits = v.to_bits();
        let sign = bits & 0x8000_0000_0000_0000;
        let exponent = ((bits >> 52) & 0x7FF) as i64;
        let mantissa_bits = bits & 0x000F_FFFF_FFFF_FFFF;

        if exponent == 0 {
            // Subnormal: normalize by multiplying by 2^53.
            let scaled = v * (1u64 << 53) as f64;
            let s_bits = scaled.to_bits();
            let s_exp = ((s_bits >> 52) & 0x7FF) as i64;
            let s_mant = s_bits & 0x000F_FFFF_FFFF_FFFF;
            let m = f64::from_bits(sign | 0x3FE0_0000_0000_0000 | s_mant);
            Ok((m, s_exp - 1022 - 53))
        } else {
            // Normal: rebias exponent to [0.5, 1) range.
            let m = f64::from_bits(sign | 0x3FE0_0000_0000_0000 | mantissa_bits);
            Ok((m, exponent - 1022))
        }
    }

    /// Return `m × 2^e`.
    ///
    /// This is the inverse of `math.frexp`: given a mantissa and
    /// an exponent, it reconstructs the original number.  The
    /// exponent `e` must be an integer.
    ///
    /// # Parameters
    ///
    /// - `m` — the mantissa
    /// - `e` — the exponent, as an integer
    ///
    /// # Returns
    ///
    /// - `m × 2^e`, as a float
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.ldexp(0.5, 4) == 8.0)
    /// local m, e = math.frexp(3.14)
    /// assert(math.ldexp(m, e) == 3.14)
    /// ```
    #[function]
    fn ldexp(m: Value, e: i64) -> Result<f64, VmError> {
        let m = to_float(m)?;
        // Clamp the exponent to avoid panicking from powi on extreme values.
        // powi takes i32, so clamp to i32 range; let overflow produce inf/zero.
        let e_clamped = e.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
        Ok(m * 2.0_f64.powi(e_clamped))
    }

    // -----------------------------------------------------------------
    // Angle conversion
    // -----------------------------------------------------------------

    /// Convert `x` from radians to degrees.
    ///
    /// Equivalent to `x * 180 / π`.
    ///
    /// # Parameters
    ///
    /// - `x` — angle in radians
    ///
    /// # Returns
    ///
    /// - the angle in degrees, as a float
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.deg(math.pi) == 180.0)
    /// assert(math.deg(0) == 0.0)
    /// ```
    #[function]
    fn deg(x: Value) -> Result<f64, VmError> {
        Ok(to_float(x)? * (180.0 / std::f64::consts::PI))
    }

    /// Convert `x` from degrees to radians.
    ///
    /// Equivalent to `x * π / 180`.
    ///
    /// # Parameters
    ///
    /// - `x` — angle in degrees
    ///
    /// # Returns
    ///
    /// - the angle in radians, as a float
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.rad(180) == math.pi)
    /// assert(math.rad(0) == 0.0)
    /// ```
    #[function]
    fn rad(x: Value) -> Result<f64, VmError> {
        Ok(to_float(x)? * (std::f64::consts::PI / 180.0))
    }

    // -----------------------------------------------------------------
    // Trigonometric
    // -----------------------------------------------------------------

    /// Return the sine of `x`, in radians.
    ///
    /// # Parameters
    ///
    /// - `x` — the angle in radians
    ///
    /// # Returns
    ///
    /// - the sine of `x`, as a float
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.sin(0) == 0.0)
    /// print(math.sin(math.pi / 2))    --> 1.0 (approximately)
    /// ```
    #[function]
    fn sin(x: Value) -> Result<f64, VmError> {
        Ok(to_float(x)?.sin())
    }

    /// Return the cosine of `x`, in radians.
    ///
    /// # Parameters
    ///
    /// - `x` — the angle in radians
    ///
    /// # Returns
    ///
    /// - the cosine of `x`, as a float
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.cos(0) == 1.0)
    /// print(math.cos(math.pi))        --> -1.0 (approximately)
    /// ```
    #[function]
    fn cos(x: Value) -> Result<f64, VmError> {
        Ok(to_float(x)?.cos())
    }

    /// Return the tangent of `x`, in radians.
    ///
    /// Tangent is undefined at odd multiples of π/2; near those
    /// points the result becomes very large in magnitude.
    ///
    /// # Parameters
    ///
    /// - `x` — the angle in radians
    ///
    /// # Returns
    ///
    /// - the tangent of `x`, as a float
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.tan(0) == 0.0)
    /// print(math.tan(math.pi / 4))    --> 1.0 (approximately)
    /// ```
    #[function]
    fn tan(x: Value) -> Result<f64, VmError> {
        Ok(to_float(x)?.tan())
    }

    /// Return the arc sine (inverse sine) of `x`, in radians.
    ///
    /// `x` must be in the range `[-1, 1]`; outside this range the
    /// result is NaN.  The returned angle is in `[-π/2, π/2]`.
    ///
    /// # Parameters
    ///
    /// - `x` — a number in `[-1, 1]`
    ///
    /// # Returns
    ///
    /// - the arc sine, in radians, as a float
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.asin(0) == 0.0)
    /// print(math.asin(1))             --> 1.5707963267949 (π/2)
    /// ```
    #[function]
    fn asin(x: Value) -> Result<f64, VmError> {
        Ok(to_float(x)?.asin())
    }

    /// Return the arc cosine (inverse cosine) of `x`, in radians.
    ///
    /// `x` must be in the range `[-1, 1]`; outside this range the
    /// result is NaN.  The returned angle is in `[0, π]`.
    ///
    /// # Parameters
    ///
    /// - `x` — a number in `[-1, 1]`
    ///
    /// # Returns
    ///
    /// - the arc cosine, in radians, as a float
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.acos(1) == 0.0)
    /// print(math.acos(-1))            --> 3.1415926535898 (π)
    /// ```
    #[function]
    fn acos(x: Value) -> Result<f64, VmError> {
        Ok(to_float(x)?.acos())
    }

    /// Return the arc tangent of `y` (or of `y / x`), in radians.
    ///
    /// With one argument, returns the arc tangent of `y` in the
    /// range `(-π/2, π/2)`.
    ///
    /// With two arguments, returns the angle of the point
    /// `(x, y)` from the positive x-axis (a.k.a. `atan2`), using
    /// the signs of both arguments to pick the correct quadrant.
    /// The result is in `(-π, π]`.
    ///
    /// # Parameters
    ///
    /// - `y` — the y coordinate (or the value to atan, when used
    ///   with one argument)
    /// - `x` — the x coordinate; defaults to `1`, which gives the
    ///   single-argument behaviour
    ///
    /// # Returns
    ///
    /// - the arc tangent angle, in radians, as a float
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.atan(0) == 0.0)
    /// print(math.atan(1))             --> 0.78539816339745 (π/4)
    /// ```
    ///
    /// ```lua
    /// -- Two-argument form: angle to a point.
    /// assert(math.atan(0, 1) == 0.0)
    /// print(math.atan(1, 1))          --> 0.78539816339745 (π/4)
    /// print(math.atan(0, -1))         --> 3.1415926535898 (π, second quadrant)
    /// ```
    #[function]
    fn atan(y: Value, x: Option<Value>) -> Result<f64, VmError> {
        let y = to_float(y)?;
        match x {
            Some(xv) => Ok(y.atan2(to_float(xv)?)),
            None => Ok(y.atan()),
        }
    }

    /// Return the arc tangent of `y / x`, using the signs of both
    /// arguments to determine the quadrant of the result.
    ///
    /// This is the two-argument form of arc tangent.  It is
    /// equivalent to `math.atan(y, x)` and is provided for
    /// compatibility with Lua 5.1 and Luau.
    ///
    /// # Parameters
    ///
    /// - `y` — the y coordinate
    /// - `x` — the x coordinate
    ///
    /// # Returns
    ///
    /// - the angle in radians, in the range `(-π, π]`, as a float
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.atan2(0, 1) == 0.0)
    /// assert(math.atan2(0, -1) == math.pi)
    /// assert(math.atan2(1, 0) == math.pi / 2)
    /// ```
    #[function(rename = "atan2")]
    fn atan2_compat(y: Value, x: Value) -> Result<f64, VmError> {
        Ok(to_float(y)?.atan2(to_float(x)?))
    }

    /// Return the hyperbolic sine of `x`.
    ///
    /// # Parameters
    ///
    /// - `x` — a real number
    ///
    /// # Returns
    ///
    /// - the hyperbolic sine of `x`, as a float
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.sinh(0) == 0.0)
    /// assert(math.sinh(1) > 1.0)
    /// ```
    #[function]
    fn sinh(x: Value) -> Result<f64, VmError> {
        Ok(to_float(x)?.sinh())
    }

    /// Return the hyperbolic cosine of `x`.
    ///
    /// # Parameters
    ///
    /// - `x` — a real number
    ///
    /// # Returns
    ///
    /// - the hyperbolic cosine of `x`, as a float
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.cosh(0) == 1.0)
    /// assert(math.cosh(1) > 1.0)
    /// ```
    #[function]
    fn cosh(x: Value) -> Result<f64, VmError> {
        Ok(to_float(x)?.cosh())
    }

    /// Return the hyperbolic tangent of `x`.
    ///
    /// # Parameters
    ///
    /// - `x` — a real number
    ///
    /// # Returns
    ///
    /// - the hyperbolic tangent of `x`, as a float
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.tanh(0) == 0.0)
    /// assert(math.tanh(1e10) == 1.0)
    /// ```
    #[function]
    fn tanh(x: Value) -> Result<f64, VmError> {
        Ok(to_float(x)?.tanh())
    }

    // -----------------------------------------------------------------
    // Min / max
    // -----------------------------------------------------------------

    /// Return the smallest of the supplied numbers.
    ///
    /// Compares values numerically.  All arguments must be numbers;
    /// non-number arguments raise an error.  At least one argument
    /// is required.  The integer-vs-float subtype of the chosen
    /// argument is preserved in the result.
    ///
    /// # Parameters
    ///
    /// - `...` — one or more numbers to compare
    ///
    /// # Returns
    ///
    /// - the smallest of the arguments
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.min(3, 1, 4, 1, 5) == 1)
    /// assert(math.min(-2.5, -3, 0) == -3)
    /// ```
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

    /// Convert `x` to an integer if possible.
    ///
    /// Returns the integer when `x` is already an integer or a
    /// float that represents an exact integer value within range.
    /// Returns `nil` when `x` is a non-integer float (e.g. `2.5`),
    /// outside the integer range, NaN, infinity, or any non-number
    /// type.
    ///
    /// # Parameters
    ///
    /// - `x` — the value to convert
    ///
    /// # Returns
    ///
    /// - the integer value, or `nil` when conversion isn't exact
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.tointeger(5) == 5)
    /// assert(math.tointeger(5.0) == 5)
    /// assert(math.tointeger(5.5) == nil)
    /// assert(math.tointeger("hello") == nil)
    /// ```
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

    /// Report the numeric subtype of `x`.
    ///
    /// Returns the string `"integer"` for integer values, `"float"`
    /// for float values, and `false` (not `nil`) for any other
    /// type.  Use `math.type` rather than `type(x) == "number"`
    /// when the integer/float distinction matters.
    ///
    /// # Parameters
    ///
    /// - `x` — any value
    ///
    /// # Returns
    ///
    /// - `"integer"`, `"float"`, or `false`
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.type(5) == "integer")
    /// assert(math.type(5.0) == "float")
    /// assert(math.type("hello") == false)
    /// assert(math.type(nil) == false)
    /// ```
    #[function(rename = "type")]
    fn math_type(x: Value) -> MathTypeResult {
        match x {
            Value::Integer(_) => MathTypeResult::Integer,
            Value::Float(_) => MathTypeResult::Float,
            _ => MathTypeResult::NotNumber,
        }
    }

    /// Return whether `m < n` using unsigned integer comparison.
    ///
    /// Both arguments must be integers; floats raise an error.
    /// The comparison treats the 64-bit two's-complement
    /// representations as *unsigned* values, so `-1` is larger
    /// than any positive integer.
    ///
    /// # Parameters
    ///
    /// - `m` — first integer (treated as unsigned)
    /// - `n` — second integer (treated as unsigned)
    ///
    /// # Returns
    ///
    /// - `true` if `m < n` in unsigned comparison, `false` otherwise
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.ult(1, 2) == true)
    /// assert(math.ult(2, 1) == false)
    /// assert(math.ult(-1, 1) == false)
    /// assert(math.ult(1, -1) == true)
    /// ```
    #[function]
    fn ult(m: i64, n: i64) -> bool {
        (m as u64) < (n as u64)
    }

    /// Return the largest of the supplied numbers.
    ///
    /// Compares values numerically.  All arguments must be numbers;
    /// non-number arguments raise an error.  At least one argument
    /// is required.  The integer-vs-float subtype of the chosen
    /// argument is preserved in the result.
    ///
    /// # Parameters
    ///
    /// - `...` — one or more numbers to compare
    ///
    /// # Returns
    ///
    /// - the largest of the arguments
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.max(3, 1, 4, 1, 5) == 5)
    /// assert(math.max(-2.5, -3, 0) == 0)
    /// ```
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

    /// Return the floating-point remainder of `x` divided by `y`.
    ///
    /// The result has the same sign as `x` and an absolute value
    /// less than the absolute value of `y`.  Raises an error when
    /// `y` is zero.
    ///
    /// This is different from Lua's `%` operator, which returns a
    /// result with the same sign as the divisor; `fmod` matches
    /// the conventional C `fmod` semantics.
    ///
    /// # Parameters
    ///
    /// - `x` — the dividend
    /// - `y` — the divisor; must not be zero
    ///
    /// # Returns
    ///
    /// - the remainder of `x / y`, as a float
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.fmod(7, 3) == 1)
    /// assert(math.fmod(-7, 3) == -1)
    /// assert(math.fmod(7.5, 2.5) == 0)
    /// ```
    #[function]
    fn fmod(x: f64, y: f64) -> Result<f64, VmError> {
        if y == 0.0 {
            return Err(VmError::BadArgument {
                position: 2,
                function: "fmod".to_owned(),
                expected: "non-zero number".to_owned(),
                got: "zero".to_owned(),
            });
        }
        Ok(x % y)
    }

    /// Constrain `x` to lie within `[min, max]`.
    ///
    /// Returns `min` when `x < min`, `max` when `x > max`, and
    /// otherwise `x` unchanged.  Raises an error when `min > max`.
    ///
    /// # Parameters
    ///
    /// - `x` — the value to clamp
    /// - `min` — the lower bound
    /// - `max` — the upper bound; must be `>= min`
    ///
    /// # Returns
    ///
    /// - `x` clamped to `[min, max]`
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.clamp(5, 0, 10) == 5)
    /// assert(math.clamp(-3, 0, 10) == 0)
    /// assert(math.clamp(15, 0, 10) == 10)
    /// ```
    #[function]
    fn clamp(
        x: crate::Number,
        min: crate::Number,
        max: crate::Number,
    ) -> Result<crate::Number, VmError> {
        let xf = x.into_float();
        let min_f = min.into_float();
        let max_f = max.into_float();
        if min_f > max_f {
            return Err(VmError::BadArgument {
                position: 3,
                function: "clamp".to_owned(),
                expected: "max must be >= min".to_owned(),
                got: format!("max ({max_f}) < min ({min_f})"),
            });
        }
        if xf < min_f {
            Ok(min)
        } else if xf > max_f {
            Ok(max)
        } else {
            Ok(x)
        }
    }

    /// Return the sign of `x` as an integer.
    ///
    /// Returns `1` when `x > 0`, `-1` when `x < 0`, and `0` when
    /// `x` is zero (including negative zero).
    ///
    /// # Parameters
    ///
    /// - `x` — the value to take the sign of
    ///
    /// # Returns
    ///
    /// - `1`, `-1`, or `0`
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.sign(42) == 1)
    /// assert(math.sign(-3.14) == -1)
    /// assert(math.sign(0) == 0)
    /// ```
    #[function]
    fn sign(x: crate::Number) -> i64 {
        let f = x.into_float();
        if f > 0.0 {
            1
        } else if f < 0.0 {
            -1
        } else {
            0
        }
    }

    /// Round `x` to the nearest integer, with ties away from zero.
    ///
    /// Halfway values like `2.5` round to `3` (away from zero) and
    /// `-2.5` rounds to `-3`.  This is the "banker's rounding"
    /// alternative; for floor / ceiling rounding use `math.floor`
    /// or `math.ceil`.
    ///
    /// # Parameters
    ///
    /// - `x` — the value to round
    ///
    /// # Returns
    ///
    /// - the rounded integer
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.round(2.4) == 2)
    /// assert(math.round(2.5) == 3)
    /// assert(math.round(-2.5) == -3)
    /// ```
    #[function]
    fn round(x: crate::Number) -> i64 {
        match x {
            crate::Number::Integer(i) => i,
            crate::Number::Float(f) => f.round() as i64,
        }
    }

    // -----------------------------------------------------------------
    // Interpolation
    // -----------------------------------------------------------------

    /// Linearly interpolate between `a` and `b` by fraction `t`.
    ///
    /// Returns `a + (b - a) * t`, except that when `t` is exactly
    /// `1.0` the result is exactly `b` with no floating-point
    /// rounding error.  This matches Luau's semantics.
    ///
    /// # Parameters
    ///
    /// - `a` — start value
    /// - `b` — end value
    /// - `t` — interpolation factor in `[0, 1]` (values outside
    ///   this range extrapolate)
    ///
    /// # Returns
    ///
    /// - the interpolated value, as a float
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.lerp(0, 100, 0) == 0.0)
    /// assert(math.lerp(0, 100, 1) == 100.0)
    /// assert(math.lerp(10, 20, 0.5) == 15.0)
    /// -- Values of `t` outside [0, 1] extrapolate.
    /// assert(math.lerp(0, 100, 1.5) == 150.0)
    /// assert(math.lerp(0, 100, -0.5) == -50.0)
    /// ```
    #[function]
    fn lerp(a: f64, b: f64, t: f64) -> f64 {
        if t == 1.0 {
            b
        } else {
            a + (b - a) * t
        }
    }

    /// Remap `x` from the input range `[in_min, in_max]` to the
    /// output range `[out_min, out_max]`.
    ///
    /// The result is `out_min + (x - in_min) * (out_max - out_min)
    /// / (in_max - in_min)`.  When `in_min == in_max` the result is
    /// NaN (division by zero), matching Luau behaviour.
    ///
    /// # Parameters
    ///
    /// - `x` — the value to remap
    /// - `in_min` — lower bound of the input range
    /// - `in_max` — upper bound of the input range
    /// - `out_min` — lower bound of the output range
    /// - `out_max` — upper bound of the output range
    ///
    /// # Returns
    ///
    /// - the remapped value, as a float
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.map(5, 0, 10, 0, 100) == 50.0)
    /// assert(math.map(0, -1, 1, 0, 255) == 127.5)
    /// -- A zero-width input range produces NaN.
    /// assert(math.isnan(math.map(5, 5, 5, 0, 1)))
    /// ```
    #[function]
    fn map(x: f64, in_min: f64, in_max: f64, out_min: f64, out_max: f64) -> f64 {
        out_min + (x - in_min) * (out_max - out_min) / (in_max - in_min)
    }

    // -----------------------------------------------------------------
    // Classifying predicates
    // -----------------------------------------------------------------

    /// Return whether `x` is NaN (not a number).
    ///
    /// Prefer this over `x ~= x`, which also works but is easy
    /// to overlook when reading code.
    ///
    /// # Parameters
    ///
    /// - `x` — the value to test
    ///
    /// # Returns
    ///
    /// - `true` if `x` is NaN, `false` otherwise
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.isnan(0 / 0) == true)
    /// assert(math.isnan(1) == false)
    /// assert(math.isnan(math.huge) == false)
    /// ```
    #[function(rename = "isnan")]
    fn math_isnan(x: f64) -> bool {
        x.is_nan()
    }

    /// Return whether `x` is positive or negative infinity.
    ///
    /// # Parameters
    ///
    /// - `x` — the value to test
    ///
    /// # Returns
    ///
    /// - `true` if `x` is `inf` or `-inf`, `false` otherwise
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.isinf(math.huge) == true)
    /// assert(math.isinf(-math.huge) == true)
    /// assert(math.isinf(0) == false)
    /// assert(math.isinf(math.nan) == false)
    /// ```
    #[function(rename = "isinf")]
    fn math_isinf(x: f64) -> bool {
        x.is_infinite()
    }

    /// Return whether `x` is a finite number — that is, neither
    /// infinite nor NaN.
    ///
    /// # Parameters
    ///
    /// - `x` — the value to test
    ///
    /// # Returns
    ///
    /// - `true` if `x` is a normal number, zero, or subnormal;
    ///   `false` if `x` is `inf`, `-inf`, or NaN
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(math.isfinite(0) == true)
    /// assert(math.isfinite(3.14) == true)
    /// assert(math.isfinite(math.huge) == false)
    /// assert(math.isfinite(math.nan) == false)
    /// ```
    #[function(rename = "isfinite")]
    fn math_isfinite(x: f64) -> bool {
        x.is_finite()
    }

    // RNG state lives on the GlobalEnv via the typed extension store,
    // so each env has its own deterministic stream and concurrent
    // VMs don't share seed.

    /// Return a uniformly-distributed pseudo-random number.
    ///
    /// The number of arguments controls the distribution:
    ///
    /// - With no arguments, returns a uniform float in `[0, 1)`.
    /// - With one integer `m` (must be `>= 1`), returns a uniform
    ///   integer in `[1, m]`.
    /// - With two integers `m` and `n` (must satisfy `m <= n`),
    ///   returns a uniform integer in `[m, n]`.
    ///
    /// Float arguments are accepted and truncated to integers
    /// before use.  Raises an error when the requested interval
    /// is empty (e.g. `m < 1` in the one-argument form, or
    /// `m > n` in the two-argument form).
    ///
    /// The RNG is per-environment: each `GlobalEnv` has its own
    /// deterministic stream seeded with `0` until the first call
    /// to `math.randomseed`.
    ///
    /// # Parameters
    ///
    /// - `m` — lower bound (one-arg form: upper bound, lower is 1)
    /// - `n` — upper bound (two-arg form)
    ///
    /// # Returns
    ///
    /// - a pseudo-random number per the rules above
    ///
    /// # Examples
    ///
    /// ```lua
    /// math.randomseed(42)
    /// local r = math.random()
    /// assert(r >= 0 and r < 1)
    /// ```
    ///
    /// ```lua
    /// math.randomseed(42)
    /// local roll = math.random(6)
    /// assert(roll >= 1 and roll <= 6)
    /// assert(math.type(roll) == "integer")
    /// ```
    ///
    /// ```lua
    /// math.randomseed(42)
    /// local pick = math.random(10, 20)
    /// assert(pick >= 10 and pick <= 20)
    /// ```
    #[function]
    fn random(
        ctx: crate::CallContext,
        m: Option<f64>,
        n: Option<f64>,
    ) -> Result<crate::Number, VmError> {
        let rng = ctx.global.extension_or_init::<MathRng, _>(MathRng::default);
        let mut rng = rng.0.lock();
        match (m.map(|v| v as i64), n.map(|v| v as i64)) {
            (None, None) => Ok(crate::Number::Float(rng.random_range(0.0..1.0))),
            (Some(m), None) => {
                if m < 1 {
                    return Err(runtime_error(
                        "bad argument #1 to 'random' (interval is empty)".to_owned(),
                    )
                    .with_arg_position(1));
                }
                Ok(crate::Number::Integer(rng.random_range(1..=m)))
            }
            (Some(m), Some(n)) => {
                if m > n {
                    return Err(runtime_error(
                        "bad argument #2 to 'random' (interval is empty)".to_owned(),
                    )
                    .with_arg_position(2));
                }
                Ok(crate::Number::Integer(rng.random_range(m..=n)))
            }
            (None, Some(_)) => Err(runtime_error(
                "bad argument #1 to 'random' (number expected, got nil)".to_owned(),
            )
            .with_arg_position(1)),
        }
    }

    /// Reseed the per-environment random number generator.
    ///
    /// With an explicit `x`, the RNG produces a deterministic
    /// stream starting from that seed; calling `math.randomseed`
    /// with the same seed again restarts the same stream, which is
    /// useful for reproducible tests and simulations.
    ///
    /// Without an argument, the RNG is seeded from the current
    /// wall-clock time at nanosecond resolution, which makes the
    /// output unpredictable from one run to the next.
    ///
    /// Float arguments are accepted and truncated to a 64-bit
    /// integer seed.
    ///
    /// # Parameters
    ///
    /// - `x` — seed value; defaults to the current wall-clock time
    ///
    /// # Returns
    ///
    /// - nothing
    ///
    /// # Examples
    ///
    /// ```lua
    /// -- Reproducible stream.
    /// math.randomseed(123)
    /// local a = math.random(100)
    /// math.randomseed(123)
    /// local b = math.random(100)
    /// assert(a == b)
    /// ```
    #[function]
    fn randomseed(ctx: crate::CallContext, x: Option<f64>) {
        let rng = ctx.global.extension_or_init::<MathRng, _>(MathRng::default);
        let seed = match x {
            Some(n) => n as u64,
            None => std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0),
        };
        *rng.0.lock() = StdRng::seed_from_u64(seed);
    }
}

// =========================================================================
// Random number generator state (per-env via GlobalEnv extensions)
// =========================================================================

use crate::sync::Mutex;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

/// Per-environment RNG state for `math.random` / `math.randomseed`.
///
/// Stored on the `GlobalEnv` via [`crate::GlobalEnv::extension_or_init`]
/// so each env has its own deterministic stream and concurrent VMs
/// don't share seed.  Seeded with `0` on first use; reseed via
/// `math.randomseed`.
struct MathRng(Mutex<StdRng>);

impl Default for MathRng {
    fn default() -> Self {
        MathRng(Mutex::new(StdRng::seed_from_u64(0)))
    }
}

fn runtime_error(msg: String) -> VmError {
    VmError::LuaError {
        display: msg.clone(),
        value: Value::string(msg),
    }
}

// =========================================================================
// Registration
// =========================================================================

/// Build the math library table and register it as the `math` global.
pub fn register(env: &crate::GlobalEnv) -> Result<(), VmError> {
    let table = math_mod::build_module_table(env)?;
    env.set_global("math", Value::Table(table));
    env.register_module_type("math", math_mod::module_type());
    Ok(())
}
