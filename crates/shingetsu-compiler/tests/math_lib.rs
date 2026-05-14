use std::sync::Arc;
mod common;

use common::{run_all, run_one};
use shingetsu::diagnostic::assert_diagnostics;
use shingetsu::valuevec;
use shingetsu_compiler::{CompileOptions, Compiler};
use shingetsu_vm::Value;

// math constants
// ---------------------------------------------------------------------------

#[tokio::test]
async fn math_pi() {
    k9::assert_equal!(
        run_one("return math.pi").await,
        Value::Float(std::f64::consts::PI)
    );
}

#[tokio::test]
async fn math_huge() {
    k9::assert_equal!(
        run_one("return math.huge").await,
        Value::Float(f64::INFINITY)
    );
}

#[tokio::test]
async fn math_huge_is_infinity() {
    k9::assert_equal!(
        run_one("return math.huge > 1e308").await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn math_maxinteger() {
    k9::assert_equal!(
        run_one("return math.maxinteger").await,
        Value::Integer(i64::MAX)
    );
}

#[tokio::test]
async fn math_mininteger() {
    k9::assert_equal!(
        run_one("return math.mininteger").await,
        Value::Integer(i64::MIN)
    );
}

#[tokio::test]
async fn math_maxinteger_plus_one_wraps() {
    // Adding 1 to maxinteger should wrap around (Lua integer overflow).
    k9::assert_equal!(
        run_one("return math.maxinteger + 1 == math.mininteger").await,
        Value::Boolean(true)
    );
}

// ---------------------------------------------------------------------------
// math.floor
// ---------------------------------------------------------------------------

#[tokio::test]
async fn math_floor_float() {
    k9::assert_equal!(run_one("return math.floor(3.7)").await, Value::Integer(3));
}

#[tokio::test]
async fn math_floor_negative() {
    k9::assert_equal!(run_one("return math.floor(-2.3)").await, Value::Integer(-3));
}

#[tokio::test]
async fn math_floor_integer_passthrough() {
    k9::assert_equal!(run_one("return math.floor(5)").await, Value::Integer(5));
}

#[tokio::test]
async fn math_floor_exact() {
    // Already whole float.
    k9::assert_equal!(run_one("return math.floor(4.0)").await, Value::Integer(4));
}

#[tokio::test]
async fn math_floor_huge() {
    // floor(inf) stays float since it can't be an integer.
    k9::assert_equal!(
        run_one("return math.floor(math.huge)").await,
        Value::Float(f64::INFINITY)
    );
}

// ---------------------------------------------------------------------------
// math.ceil
// ---------------------------------------------------------------------------

#[tokio::test]
async fn math_ceil_float() {
    k9::assert_equal!(run_one("return math.ceil(3.2)").await, Value::Integer(4));
}

#[tokio::test]
async fn math_ceil_negative() {
    k9::assert_equal!(run_one("return math.ceil(-2.7)").await, Value::Integer(-2));
}

#[tokio::test]
async fn math_ceil_integer_passthrough() {
    k9::assert_equal!(run_one("return math.ceil(5)").await, Value::Integer(5));
}

#[tokio::test]
async fn math_ceil_exact() {
    k9::assert_equal!(run_one("return math.ceil(4.0)").await, Value::Integer(4));
}

// ---------------------------------------------------------------------------
// math.abs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn math_abs_positive_int() {
    k9::assert_equal!(run_one("return math.abs(42)").await, Value::Integer(42));
}

#[tokio::test]
async fn math_abs_negative_int() {
    k9::assert_equal!(run_one("return math.abs(-42)").await, Value::Integer(42));
}

#[tokio::test]
async fn math_abs_float() {
    k9::assert_equal!(run_one("return math.abs(-3.14)").await, Value::Float(3.14));
}

#[tokio::test]
async fn math_abs_zero() {
    k9::assert_equal!(run_one("return math.abs(0)").await, Value::Integer(0));
}

#[tokio::test]
async fn math_abs_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.abs, 'hello') return ok").await,
        Value::Boolean(false)
    );
}

// ---------------------------------------------------------------------------
// math.modf
// ---------------------------------------------------------------------------

#[tokio::test]
async fn math_modf_positive() {
    let res = run_all("return math.modf(3.75)").await;
    k9::assert_equal!(res, valuevec![Value::Integer(3), Value::Float(0.75)]);
}

#[tokio::test]
async fn math_modf_negative() {
    let res = run_all("return math.modf(-3.75)").await;
    k9::assert_equal!(res, valuevec![Value::Integer(-3), Value::Float(-0.75)]);
}

#[tokio::test]
async fn math_modf_integer() {
    let res = run_all("return math.modf(5)").await;
    k9::assert_equal!(res, valuevec![Value::Integer(5), Value::Float(0.0)]);
}

#[tokio::test]
async fn math_modf_whole_float() {
    let res = run_all("return math.modf(4.0)").await;
    k9::assert_equal!(res, valuevec![Value::Integer(4), Value::Float(0.0)]);
}

#[tokio::test]
async fn math_modf_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.modf, 'hello') return ok").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn math_modf_infinity() {
    // modf(inf) — integral part is inf (can't be integer), frac is NaN.
    let res = run_all("return math.modf(math.huge)").await;
    k9::assert_equal!(res.len(), 2);
    k9::assert_equal!(res[0], Value::Float(f64::INFINITY));
    // inf - inf = NaN; verify via NaN ~= NaN.
    match res[1] {
        Value::Float(f) => assert!(f.is_nan(), "expected NaN, got {}", f),
        ref other => panic!("expected Float, got {:?}", other),
    }
}

#[tokio::test]
async fn math_floor_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.floor, 'hello') return ok").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn math_floor_nan() {
    // floor(NaN) stays float since NaN is not finite.
    let res = run_one("return math.floor(0/0) ~= math.floor(0/0)").await;
    // NaN ~= NaN is true in Lua.
    k9::assert_equal!(res, Value::Boolean(true));
}

#[tokio::test]
async fn math_ceil_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.ceil, 'hello') return ok").await,
        Value::Boolean(false)
    );
}

// ---------------------------------------------------------------------------
// math.sqrt
// ---------------------------------------------------------------------------

#[tokio::test]
async fn math_sqrt_perfect() {
    k9::assert_equal!(run_one("return math.sqrt(9)").await, Value::Float(3.0));
}

#[tokio::test]
async fn math_sqrt_float() {
    k9::assert_equal!(
        run_one("return math.sqrt(2.0)").await,
        Value::Float(2.0_f64.sqrt())
    );
}

#[tokio::test]
async fn math_sqrt_zero() {
    k9::assert_equal!(run_one("return math.sqrt(0)").await, Value::Float(0.0));
}

#[tokio::test]
async fn math_sqrt_negative_is_nan() {
    k9::assert_equal!(
        run_one("return math.sqrt(-1) ~= math.sqrt(-1)").await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn math_sqrt_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.sqrt, {}) return ok").await,
        Value::Boolean(false)
    );
}

// ---------------------------------------------------------------------------
// math.exp
// ---------------------------------------------------------------------------

#[tokio::test]
async fn math_exp_zero() {
    k9::assert_equal!(run_one("return math.exp(0)").await, Value::Float(1.0));
}

#[tokio::test]
async fn math_exp_one() {
    k9::assert_equal!(
        run_one("return math.exp(1)").await,
        Value::Float(std::f64::consts::E)
    );
}

#[tokio::test]
async fn math_exp_negative() {
    k9::assert_equal!(
        run_one("return math.exp(-1)").await,
        Value::Float((-1.0_f64).exp())
    );
}

#[tokio::test]
async fn math_exp_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.exp, true) return ok").await,
        Value::Boolean(false)
    );
}

// ---------------------------------------------------------------------------
// math.log
// ---------------------------------------------------------------------------

#[tokio::test]
async fn math_log_natural() {
    k9::assert_equal!(run_one("return math.log(1)").await, Value::Float(0.0));
}

#[tokio::test]
async fn math_log_e() {
    // ln(e) == 1
    k9::assert_equal!(
        run_one("return math.log(math.exp(1))").await,
        Value::Float(1.0)
    );
}

#[tokio::test]
async fn math_log_base_10() {
    // ln(1000)/ln(10) has floating-point rounding, so compare approximately.
    k9::assert_equal!(
        run_one("return math.log(1000, 10) > 2.999 and math.log(1000, 10) < 3.001").await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn math_log_base_2() {
    k9::assert_equal!(run_one("return math.log(8, 2)").await, Value::Float(3.0));
}

#[tokio::test]
async fn math_log_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.log, 'hello') return ok").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn math_log_bad_base_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.log, 10, 'hello') return ok").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn math_log_float_input() {
    k9::assert_equal!(run_one("return math.log(1.0)").await, Value::Float(0.0));
}

// ---------------------------------------------------------------------------
// math.sin / math.cos / math.tan
// ---------------------------------------------------------------------------

#[tokio::test]
async fn math_sin_zero() {
    k9::assert_equal!(run_one("return math.sin(0)").await, Value::Float(0.0));
}

#[tokio::test]
async fn math_sin_pi_half() {
    k9::assert_equal!(
        run_one("return math.sin(math.pi / 2)").await,
        Value::Float((std::f64::consts::PI / 2.0).sin())
    );
}

#[tokio::test]
async fn math_sin_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.sin, 'x') return ok").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn math_cos_zero() {
    k9::assert_equal!(run_one("return math.cos(0)").await, Value::Float(1.0));
}

#[tokio::test]
async fn math_cos_pi() {
    k9::assert_equal!(
        run_one("return math.cos(math.pi)").await,
        Value::Float(std::f64::consts::PI.cos())
    );
}

#[tokio::test]
async fn math_tan_zero() {
    k9::assert_equal!(run_one("return math.tan(0)").await, Value::Float(0.0));
}

#[tokio::test]
async fn math_tan_pi_quarter() {
    k9::assert_equal!(
        run_one("return math.tan(math.pi / 4)").await,
        Value::Float((std::f64::consts::PI / 4.0).tan())
    );
}

// ---------------------------------------------------------------------------
// math.asin / math.acos / math.atan
// ---------------------------------------------------------------------------

#[tokio::test]
async fn math_asin_zero() {
    k9::assert_equal!(run_one("return math.asin(0)").await, Value::Float(0.0));
}

#[tokio::test]
async fn math_asin_one() {
    k9::assert_equal!(
        run_one("return math.asin(1)").await,
        Value::Float(1.0_f64.asin())
    );
}

#[tokio::test]
async fn math_asin_out_of_range_is_nan() {
    // asin(2) is NaN.
    k9::assert_equal!(
        run_one("return math.asin(2) ~= math.asin(2)").await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn math_acos_one() {
    k9::assert_equal!(run_one("return math.acos(1)").await, Value::Float(0.0));
}

#[tokio::test]
async fn math_acos_zero() {
    k9::assert_equal!(
        run_one("return math.acos(0)").await,
        Value::Float(0.0_f64.acos())
    );
}

#[tokio::test]
async fn math_atan_zero() {
    k9::assert_equal!(run_one("return math.atan(0)").await, Value::Float(0.0));
}

#[tokio::test]
async fn math_atan_one() {
    k9::assert_equal!(
        run_one("return math.atan(1)").await,
        Value::Float(1.0_f64.atan())
    );
}

#[tokio::test]
async fn math_atan_two_args() {
    // atan(1, 1) == atan2(1, 1) == pi/4
    k9::assert_equal!(
        run_one("return math.atan(1, 1)").await,
        Value::Float(1.0_f64.atan2(1.0))
    );
}

#[tokio::test]
async fn math_atan_two_args_negative() {
    // atan(-1, -1) == atan2(-1, -1) == -3*pi/4
    k9::assert_equal!(
        run_one("return math.atan(-1, -1)").await,
        Value::Float((-1.0_f64).atan2(-1.0))
    );
}

#[tokio::test]
async fn math_atan_bad_second_arg() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.atan, 1, 'x') return ok").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn math_trig_roundtrip() {
    // asin(sin(x)) should return x for x in [-pi/2, pi/2].
    k9::assert_equal!(
        run_one("return math.asin(math.sin(0.5)) - 0.5 < 1e-10").await,
        Value::Boolean(true)
    );
}

// ---------------------------------------------------------------------------
// math.min
// ---------------------------------------------------------------------------

#[tokio::test]
async fn math_min_two() {
    k9::assert_equal!(run_one("return math.min(3, 1)").await, Value::Integer(1));
}

#[tokio::test]
async fn math_min_many() {
    k9::assert_equal!(
        run_one("return math.min(5, 3, 8, 1, 4)").await,
        Value::Integer(1)
    );
}

#[tokio::test]
async fn math_min_single() {
    k9::assert_equal!(run_one("return math.min(42)").await, Value::Integer(42));
}

#[tokio::test]
async fn math_min_negative() {
    k9::assert_equal!(
        run_one("return math.min(-10, -20, -5)").await,
        Value::Integer(-20)
    );
}

#[tokio::test]
async fn math_min_mixed_int_float() {
    // Should return the float since 1.5 < 2.
    k9::assert_equal!(run_one("return math.min(2, 1.5)").await, Value::Float(1.5));
}

#[tokio::test]
async fn math_min_preserves_integer() {
    // When the minimum is an integer, it stays integer.
    k9::assert_equal!(run_one("return math.min(1, 2.5)").await, Value::Integer(1));
}

#[tokio::test]
async fn math_min_no_args() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.min) return ok").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn math_min_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.min, 1, 'x') return ok").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn math_min_tie_returns_first() {
    // Equal values: the first argument wins, preserving its type.
    k9::assert_equal!(run_one("return math.min(1, 1.0)").await, Value::Integer(1));
}

// ---------------------------------------------------------------------------
// math.max
// ---------------------------------------------------------------------------

#[tokio::test]
async fn math_max_two() {
    k9::assert_equal!(run_one("return math.max(3, 1)").await, Value::Integer(3));
}

#[tokio::test]
async fn math_max_many() {
    k9::assert_equal!(
        run_one("return math.max(5, 3, 8, 1, 4)").await,
        Value::Integer(8)
    );
}

#[tokio::test]
async fn math_max_single() {
    k9::assert_equal!(run_one("return math.max(42)").await, Value::Integer(42));
}

#[tokio::test]
async fn math_max_negative() {
    k9::assert_equal!(
        run_one("return math.max(-10, -20, -5)").await,
        Value::Integer(-5)
    );
}

#[tokio::test]
async fn math_max_mixed_int_float() {
    // Should return the float since 3.5 > 2.
    k9::assert_equal!(run_one("return math.max(2, 3.5)").await, Value::Float(3.5));
}

#[tokio::test]
async fn math_max_preserves_integer() {
    k9::assert_equal!(run_one("return math.max(3, 1.5)").await, Value::Integer(3));
}

#[tokio::test]
async fn math_max_no_args() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.max) return ok").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn math_max_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.max, 1, true) return ok").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn math_max_tie_returns_first() {
    // Equal values: the first argument wins, preserving its type.
    k9::assert_equal!(run_one("return math.max(1, 1.0)").await, Value::Integer(1));
}

// ---------------------------------------------------------------------------
// math.tointeger
// ---------------------------------------------------------------------------

#[tokio::test]
async fn math_tointeger_from_integer() {
    k9::assert_equal!(
        run_one("return math.tointeger(42)").await,
        Value::Integer(42)
    );
}

#[tokio::test]
async fn math_tointeger_from_whole_float() {
    k9::assert_equal!(
        run_one("return math.tointeger(5.0)").await,
        Value::Integer(5)
    );
}

#[tokio::test]
async fn math_tointeger_fractional_fails() {
    k9::assert_equal!(run_one("return math.tointeger(3.14)").await, Value::Nil);
}

#[tokio::test]
async fn math_tointeger_infinity_fails() {
    k9::assert_equal!(
        run_one("return math.tointeger(math.huge)").await,
        Value::Nil
    );
}

#[tokio::test]
async fn math_tointeger_nan_fails() {
    k9::assert_equal!(run_one("return math.tointeger(0/0)").await, Value::Nil);
}

#[tokio::test]
async fn math_tointeger_string_fails() {
    k9::assert_equal!(run_one("return math.tointeger('42')").await, Value::Nil);
}

#[tokio::test]
async fn math_tointeger_nil_fails() {
    k9::assert_equal!(run_one("return math.tointeger(nil)").await, Value::Nil);
}

#[tokio::test]
async fn math_tointeger_negative() {
    k9::assert_equal!(
        run_one("return math.tointeger(-7.0)").await,
        Value::Integer(-7)
    );
}

// ---------------------------------------------------------------------------
// math.type
// ---------------------------------------------------------------------------

#[tokio::test]
async fn math_type_integer() {
    k9::assert_equal!(
        run_one("return math.type(42)").await,
        Value::string("integer")
    );
}

#[tokio::test]
async fn math_type_float() {
    k9::assert_equal!(
        run_one("return math.type(3.14)").await,
        Value::string("float")
    );
}

#[tokio::test]
async fn math_type_string() {
    k9::assert_equal!(
        run_one("return math.type('hello')").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn math_type_nil() {
    k9::assert_equal!(
        run_one("return math.type(nil)").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn math_type_boolean() {
    k9::assert_equal!(
        run_one("return math.type(true)").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn math_type_integer_zero() {
    // 0 is an integer, not a float.
    k9::assert_equal!(
        run_one("return math.type(0)").await,
        Value::string("integer")
    );
}

#[tokio::test]
async fn math_type_float_zero() {
    k9::assert_equal!(
        run_one("return math.type(0.0)").await,
        Value::string("float")
    );
}

// ---------------------------------------------------------------------------
// math.random
// ---------------------------------------------------------------------------

#[tokio::test]
async fn math_random_no_args_returns_float() {
    // No args: returns a float in [0, 1).
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed(42)
            local r = math.random()
            return type(r) == 'number' and r >= 0 and r < 1"
        )
        .await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn math_random_no_args_is_float_type() {
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed(42)
            return math.type(math.random())"
        )
        .await,
        Value::string("float")
    );
}

#[tokio::test]
async fn math_random_one_arg() {
    // One arg m: returns an integer in [1, m].
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed(42)
            local r = math.random(10)
            return math.type(r) == 'integer' and r >= 1 and r <= 10"
        )
        .await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn math_random_two_args() {
    // Two args: returns an integer in [m, n].
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed(42)
            local r = math.random(5, 10)
            return math.type(r) == 'integer' and r >= 5 and r <= 10"
        )
        .await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn math_random_one_arg_is_one() {
    // math.random(1) always returns 1.
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed(42)
            return math.random(1)"
        )
        .await,
        Value::Integer(1)
    );
}

#[tokio::test]
async fn math_random_same_bounds() {
    // math.random(5, 5) always returns 5.
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed(42)
            return math.random(5, 5)"
        )
        .await,
        Value::Integer(5)
    );
}

#[tokio::test]
async fn math_random_empty_interval_one_arg() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.random, 0) return ok").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn math_random_empty_interval_two_args() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.random, 10, 5) return ok").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn math_random_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.random, 'x') return ok").await,
        Value::Boolean(false)
    );
}

// ---------------------------------------------------------------------------
// math.randomseed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn math_randomseed_deterministic() {
    // Same seed produces the same sequence.
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed(123)
            local a = math.random()
            math.randomseed(123)
            local b = math.random()
            return a == b"
        )
        .await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn math_randomseed_no_args() {
    // No args uses a time-based seed; just check it doesn't error.
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed()
            local r = math.random()
            return r >= 0 and r < 1"
        )
        .await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn math_randomseed_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.randomseed, 'x') return ok").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn math_random_negative_one_arg() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.random, -1) return ok").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn math_random_float_coercion() {
    // 10.5 truncates to 10; result should be in [1, 10].
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed(42)
            local r = math.random(10.5)
            return r >= 1 and r <= 10"
        )
        .await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn math_random_sequence_varies() {
    // Multiple calls should not all return the same value.
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed(42)
            local a = math.random()
            local b = math.random()
            local c = math.random()
            return a ~= b or b ~= c"
        )
        .await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn math_randomseed_different_seeds_diverge() {
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed(1)
            local a = math.random()
            math.randomseed(2)
            local b = math.random()
            return a ~= b"
        )
        .await,
        Value::Boolean(true)
    );
}

// ---------------------------------------------------------------------------
// math.fmod
// ---------------------------------------------------------------------------

#[tokio::test]
async fn math_fmod_basic() {
    k9::assert_equal!(run_one("return math.fmod(7, 3)").await, Value::Float(1.0));
}

#[tokio::test]
async fn math_fmod_negative() {
    k9::assert_equal!(run_one("return math.fmod(-7, 3)").await, Value::Float(-1.0));
}

#[tokio::test]
async fn math_fmod_float() {
    k9::assert_equal!(
        run_one("return math.fmod(7.5, 2.0)").await,
        Value::Float(1.5)
    );
}

#[tokio::test]
async fn math_fmod_zero_divisor_errors() {
    common::assert_runtime_error!(
        "return math.fmod(1, 0)",
        "\
error: bad argument #2 to 'fmod' (non-zero number expected, got zero)
 --> test.lua:1:21
  |
1 | return math.fmod(1, 0)
  |                     ^ bad argument #2 to 'fmod' (non-zero number expected, got zero)
stack traceback:
\ttest.lua:1: in main chunk",
    );
}

// ---------------------------------------------------------------------------
// math.clamp
// ---------------------------------------------------------------------------

#[tokio::test]
async fn math_clamp_within_range() {
    k9::assert_equal!(
        run_one("return math.clamp(5, 1, 10)").await,
        Value::Integer(5)
    );
}

#[tokio::test]
async fn math_clamp_below_min() {
    k9::assert_equal!(
        run_one("return math.clamp(-3, 0, 10)").await,
        Value::Integer(0)
    );
}

#[tokio::test]
async fn math_clamp_above_max() {
    k9::assert_equal!(
        run_one("return math.clamp(99, 0, 10)").await,
        Value::Integer(10)
    );
}

#[tokio::test]
async fn math_clamp_float() {
    k9::assert_equal!(
        run_one("return math.clamp(1.5, 2.0, 3.0)").await,
        Value::Float(2.0)
    );
}

#[tokio::test]
async fn math_clamp_equal_bounds() {
    k9::assert_equal!(
        run_one("return math.clamp(99, 5, 5)").await,
        Value::Integer(5)
    );
}

#[tokio::test]
async fn math_clamp_invalid_range_errors() {
    common::assert_runtime_error!(
        "return math.clamp(1, 10, 5)",
        "\
error: bad argument #3 to 'clamp' (max must be >= min expected, got max (5) < min (10))
 --> test.lua:1:26
  |
1 | return math.clamp(1, 10, 5)
  |                          ^ bad argument #3 to 'clamp' (max must be >= min expected, got max (5) < min (10))
stack traceback:
\ttest.lua:1: in main chunk",
    );
}

// ---------------------------------------------------------------------------
// math.sign
// ---------------------------------------------------------------------------

#[tokio::test]
async fn math_sign_positive() {
    k9::assert_equal!(run_one("return math.sign(42)").await, Value::Integer(1));
}

#[tokio::test]
async fn math_sign_negative() {
    k9::assert_equal!(run_one("return math.sign(-3.5)").await, Value::Integer(-1));
}

#[tokio::test]
async fn math_sign_zero() {
    k9::assert_equal!(run_one("return math.sign(0)").await, Value::Integer(0));
}

// ---------------------------------------------------------------------------
// math.round
// ---------------------------------------------------------------------------

#[tokio::test]
async fn math_round_down() {
    k9::assert_equal!(run_one("return math.round(2.3)").await, Value::Integer(2));
}

#[tokio::test]
async fn math_round_up() {
    k9::assert_equal!(run_one("return math.round(2.7)").await, Value::Integer(3));
}

#[tokio::test]
async fn math_round_half() {
    k9::assert_equal!(run_one("return math.round(2.5)").await, Value::Integer(3));
}

#[tokio::test]
async fn math_round_negative_half() {
    k9::assert_equal!(run_one("return math.round(-2.5)").await, Value::Integer(-3));
}

#[tokio::test]
async fn math_round_integer_passthrough() {
    k9::assert_equal!(run_one("return math.round(7)").await, Value::Integer(7));
}

// Type checker tests
// ---------------------------------------------------------------------------

fn type_check_compiler() -> Compiler {
    let env = shingetsu_vm::GlobalEnv::new();
    shingetsu::register_libs(&env, shingetsu::Libraries::ALL).expect("register");
    Compiler::new(
        CompileOptions {
            debug_info: true,
            source_name: Arc::new("@test.lua".to_string()),
            type_check: true,
        },
        env.global_type_map(),
    )
}

#[tokio::test]
async fn type_check_fmod_correct_usage() {
    let compiler = type_check_compiler();
    let src = "return math.fmod(10, 3)";
    let bc = compiler.compile(src).await.expect("compile");
    assert_diagnostics(&bc.diagnostics, src, "");
}

#[tokio::test]
async fn type_check_fmod_too_few_args() {
    let compiler = type_check_compiler();
    let src = "math.fmod(1)";
    let bc = compiler.compile(src).await.expect("compile");
    assert_diagnostics(
        &bc.diagnostics,
        src,
        "\
error[arg_count]: expected 2 arguments but got 1
 --> test.lua:1:10
  |
1 | math.fmod(1)
  |          ^^^ expected 2 arguments but got 1",
    );
}

#[tokio::test]
async fn type_check_fmod_too_many_args() {
    let compiler = type_check_compiler();
    let src = "math.fmod(1, 2, 3)";
    let bc = compiler.compile(src).await.expect("compile");
    assert_diagnostics(
        &bc.diagnostics,
        src,
        "\
error[arg_count]: expected 2 arguments but got 3
 --> test.lua:1:10
  |
1 | math.fmod(1, 2, 3)
  |          ^^^^^^^^^ expected 2 arguments but got 3",
    );
}

#[tokio::test]
async fn type_check_fmod_wrong_type() {
    let compiler = type_check_compiler();
    let src = r#"math.fmod("hello", 2)"#;
    let bc = compiler.compile(src).await.expect("compile");
    assert_diagnostics(
        &bc.diagnostics,
        src,
        "\
error[arg_type]: expected 'number' for parameter 'x' but got 'string'
 --> test.lua:1:11
  |
1 | math.fmod(\"hello\", 2)
  |           ^^^^^^^ expected 'number' for parameter 'x' but got 'string'",
    );
}

#[tokio::test]
async fn type_check_clamp_correct_usage() {
    let compiler = type_check_compiler();
    let src = "return math.clamp(5, 1, 10)";
    let bc = compiler.compile(src).await.expect("compile");
    assert_diagnostics(&bc.diagnostics, src, "");
}

#[tokio::test]
async fn type_check_clamp_too_few_args() {
    let compiler = type_check_compiler();
    let src = "math.clamp(1)";
    let bc = compiler.compile(src).await.expect("compile");
    assert_diagnostics(
        &bc.diagnostics,
        src,
        "\
error[arg_count]: expected 3 arguments but got 1
 --> test.lua:1:11
  |
1 | math.clamp(1)
  |           ^^^ expected 3 arguments but got 1",
    );
}

#[tokio::test]
async fn type_check_sign_correct_usage() {
    let compiler = type_check_compiler();
    let src = "return math.sign(-5)";
    let bc = compiler.compile(src).await.expect("compile");
    assert_diagnostics(&bc.diagnostics, src, "");
}

#[tokio::test]
async fn type_check_sign_wrong_type() {
    let compiler = type_check_compiler();
    let src = r#"math.sign("abc")"#;
    let bc = compiler.compile(src).await.expect("compile");
    assert_diagnostics(
        &bc.diagnostics,
        src,
        "\
error[arg_type]: expected 'number' for parameter 'x' but got 'string'
 --> test.lua:1:11
  |
1 | math.sign(\"abc\")
  |           ^^^^^ expected 'number' for parameter 'x' but got 'string'",
    );
}

#[tokio::test]
async fn type_check_round_correct_usage() {
    let compiler = type_check_compiler();
    let src = "return math.round(3.7)";
    let bc = compiler.compile(src).await.expect("compile");
    assert_diagnostics(&bc.diagnostics, src, "");
}

#[tokio::test]
async fn type_check_round_wrong_type() {
    let compiler = type_check_compiler();
    let src = r#"math.round(true)"#;
    let bc = compiler.compile(src).await.expect("compile");
    assert_diagnostics(
        &bc.diagnostics,
        src,
        "\
error[arg_type]: expected 'number' for parameter 'x' but got 'boolean'
 --> test.lua:1:12
  |
1 | math.round(true)
  |            ^^^^ expected 'number' for parameter 'x' but got 'boolean'",
    );
}
