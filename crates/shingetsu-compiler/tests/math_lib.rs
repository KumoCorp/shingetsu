mod common;

use common::{run_all, run_one};
use shingetsu_vm::Value;

// math constants
// ---------------------------------------------------------------------------

#[test]
fn math_pi() {
    k9::assert_equal!(
        run_one("return math.pi"),
        Value::Float(std::f64::consts::PI)
    );
}

#[test]
fn math_huge() {
    k9::assert_equal!(run_one("return math.huge"), Value::Float(f64::INFINITY));
}

#[test]
fn math_huge_is_infinity() {
    k9::assert_equal!(run_one("return math.huge > 1e308"), Value::Boolean(true));
}

#[test]
fn math_maxinteger() {
    k9::assert_equal!(run_one("return math.maxinteger"), Value::Integer(i64::MAX));
}

#[test]
fn math_mininteger() {
    k9::assert_equal!(run_one("return math.mininteger"), Value::Integer(i64::MIN));
}

#[test]
fn math_maxinteger_plus_one_wraps() {
    // Adding 1 to maxinteger should wrap around (Lua integer overflow).
    k9::assert_equal!(
        run_one("return math.maxinteger + 1 == math.mininteger"),
        Value::Boolean(true)
    );
}

// ---------------------------------------------------------------------------
// math.floor
// ---------------------------------------------------------------------------

#[test]
fn math_floor_float() {
    k9::assert_equal!(run_one("return math.floor(3.7)"), Value::Integer(3));
}

#[test]
fn math_floor_negative() {
    k9::assert_equal!(run_one("return math.floor(-2.3)"), Value::Integer(-3));
}

#[test]
fn math_floor_integer_passthrough() {
    k9::assert_equal!(run_one("return math.floor(5)"), Value::Integer(5));
}

#[test]
fn math_floor_exact() {
    // Already whole float.
    k9::assert_equal!(run_one("return math.floor(4.0)"), Value::Integer(4));
}

#[test]
fn math_floor_huge() {
    // floor(inf) stays float since it can't be an integer.
    k9::assert_equal!(
        run_one("return math.floor(math.huge)"),
        Value::Float(f64::INFINITY)
    );
}

// ---------------------------------------------------------------------------
// math.ceil
// ---------------------------------------------------------------------------

#[test]
fn math_ceil_float() {
    k9::assert_equal!(run_one("return math.ceil(3.2)"), Value::Integer(4));
}

#[test]
fn math_ceil_negative() {
    k9::assert_equal!(run_one("return math.ceil(-2.7)"), Value::Integer(-2));
}

#[test]
fn math_ceil_integer_passthrough() {
    k9::assert_equal!(run_one("return math.ceil(5)"), Value::Integer(5));
}

#[test]
fn math_ceil_exact() {
    k9::assert_equal!(run_one("return math.ceil(4.0)"), Value::Integer(4));
}

// ---------------------------------------------------------------------------
// math.abs
// ---------------------------------------------------------------------------

#[test]
fn math_abs_positive_int() {
    k9::assert_equal!(run_one("return math.abs(42)"), Value::Integer(42));
}

#[test]
fn math_abs_negative_int() {
    k9::assert_equal!(run_one("return math.abs(-42)"), Value::Integer(42));
}

#[test]
fn math_abs_float() {
    k9::assert_equal!(run_one("return math.abs(-3.14)"), Value::Float(3.14));
}

#[test]
fn math_abs_zero() {
    k9::assert_equal!(run_one("return math.abs(0)"), Value::Integer(0));
}

#[test]
fn math_abs_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.abs, 'hello') return ok"),
        Value::Boolean(false)
    );
}

// ---------------------------------------------------------------------------
// math.modf
// ---------------------------------------------------------------------------

#[test]
fn math_modf_positive() {
    let res = run_all("return math.modf(3.75)");
    k9::assert_equal!(res, vec![Value::Integer(3), Value::Float(0.75)]);
}

#[test]
fn math_modf_negative() {
    let res = run_all("return math.modf(-3.75)");
    k9::assert_equal!(res, vec![Value::Integer(-3), Value::Float(-0.75)]);
}

#[test]
fn math_modf_integer() {
    let res = run_all("return math.modf(5)");
    k9::assert_equal!(res, vec![Value::Integer(5), Value::Float(0.0)]);
}

#[test]
fn math_modf_whole_float() {
    let res = run_all("return math.modf(4.0)");
    k9::assert_equal!(res, vec![Value::Integer(4), Value::Float(0.0)]);
}

#[test]
fn math_modf_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.modf, 'hello') return ok"),
        Value::Boolean(false)
    );
}

#[test]
fn math_modf_infinity() {
    // modf(inf) — integral part is inf (can't be integer), frac is NaN.
    let res = run_all("return math.modf(math.huge)");
    k9::assert_equal!(res.len(), 2);
    k9::assert_equal!(res[0], Value::Float(f64::INFINITY));
    // inf - inf = NaN; verify via NaN ~= NaN.
    match res[1] {
        Value::Float(f) => assert!(f.is_nan(), "expected NaN, got {}", f),
        ref other => panic!("expected Float, got {:?}", other),
    }
}

#[test]
fn math_floor_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.floor, 'hello') return ok"),
        Value::Boolean(false)
    );
}

#[test]
fn math_floor_nan() {
    // floor(NaN) stays float since NaN is not finite.
    let res = run_one("return math.floor(0/0) ~= math.floor(0/0)");
    // NaN ~= NaN is true in Lua.
    k9::assert_equal!(res, Value::Boolean(true));
}

#[test]
fn math_ceil_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.ceil, 'hello') return ok"),
        Value::Boolean(false)
    );
}

// ---------------------------------------------------------------------------
// math.sqrt
// ---------------------------------------------------------------------------

#[test]
fn math_sqrt_perfect() {
    k9::assert_equal!(run_one("return math.sqrt(9)"), Value::Float(3.0));
}

#[test]
fn math_sqrt_float() {
    k9::assert_equal!(
        run_one("return math.sqrt(2.0)"),
        Value::Float(2.0_f64.sqrt())
    );
}

#[test]
fn math_sqrt_zero() {
    k9::assert_equal!(run_one("return math.sqrt(0)"), Value::Float(0.0));
}

#[test]
fn math_sqrt_negative_is_nan() {
    k9::assert_equal!(
        run_one("return math.sqrt(-1) ~= math.sqrt(-1)"),
        Value::Boolean(true)
    );
}

#[test]
fn math_sqrt_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.sqrt, {}) return ok"),
        Value::Boolean(false)
    );
}

// ---------------------------------------------------------------------------
// math.exp
// ---------------------------------------------------------------------------

#[test]
fn math_exp_zero() {
    k9::assert_equal!(run_one("return math.exp(0)"), Value::Float(1.0));
}

#[test]
fn math_exp_one() {
    k9::assert_equal!(
        run_one("return math.exp(1)"),
        Value::Float(std::f64::consts::E)
    );
}

#[test]
fn math_exp_negative() {
    k9::assert_equal!(
        run_one("return math.exp(-1)"),
        Value::Float((-1.0_f64).exp())
    );
}

#[test]
fn math_exp_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.exp, true) return ok"),
        Value::Boolean(false)
    );
}

// ---------------------------------------------------------------------------
// math.log
// ---------------------------------------------------------------------------

#[test]
fn math_log_natural() {
    k9::assert_equal!(run_one("return math.log(1)"), Value::Float(0.0));
}

#[test]
fn math_log_e() {
    // ln(e) == 1
    k9::assert_equal!(run_one("return math.log(math.exp(1))"), Value::Float(1.0));
}

#[test]
fn math_log_base_10() {
    // ln(1000)/ln(10) has floating-point rounding, so compare approximately.
    k9::assert_equal!(
        run_one("return math.log(1000, 10) > 2.999 and math.log(1000, 10) < 3.001"),
        Value::Boolean(true)
    );
}

#[test]
fn math_log_base_2() {
    k9::assert_equal!(run_one("return math.log(8, 2)"), Value::Float(3.0));
}

#[test]
fn math_log_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.log, 'hello') return ok"),
        Value::Boolean(false)
    );
}

#[test]
fn math_log_bad_base_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.log, 10, 'hello') return ok"),
        Value::Boolean(false)
    );
}

#[test]
fn math_log_float_input() {
    k9::assert_equal!(run_one("return math.log(1.0)"), Value::Float(0.0));
}

// ---------------------------------------------------------------------------
// math.sin / math.cos / math.tan
// ---------------------------------------------------------------------------

#[test]
fn math_sin_zero() {
    k9::assert_equal!(run_one("return math.sin(0)"), Value::Float(0.0));
}

#[test]
fn math_sin_pi_half() {
    k9::assert_equal!(
        run_one("return math.sin(math.pi / 2)"),
        Value::Float((std::f64::consts::PI / 2.0).sin())
    );
}

#[test]
fn math_sin_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.sin, 'x') return ok"),
        Value::Boolean(false)
    );
}

#[test]
fn math_cos_zero() {
    k9::assert_equal!(run_one("return math.cos(0)"), Value::Float(1.0));
}

#[test]
fn math_cos_pi() {
    k9::assert_equal!(
        run_one("return math.cos(math.pi)"),
        Value::Float(std::f64::consts::PI.cos())
    );
}

#[test]
fn math_tan_zero() {
    k9::assert_equal!(run_one("return math.tan(0)"), Value::Float(0.0));
}

#[test]
fn math_tan_pi_quarter() {
    k9::assert_equal!(
        run_one("return math.tan(math.pi / 4)"),
        Value::Float((std::f64::consts::PI / 4.0).tan())
    );
}

// ---------------------------------------------------------------------------
// math.asin / math.acos / math.atan
// ---------------------------------------------------------------------------

#[test]
fn math_asin_zero() {
    k9::assert_equal!(run_one("return math.asin(0)"), Value::Float(0.0));
}

#[test]
fn math_asin_one() {
    k9::assert_equal!(run_one("return math.asin(1)"), Value::Float(1.0_f64.asin()));
}

#[test]
fn math_asin_out_of_range_is_nan() {
    // asin(2) is NaN.
    k9::assert_equal!(
        run_one("return math.asin(2) ~= math.asin(2)"),
        Value::Boolean(true)
    );
}

#[test]
fn math_acos_one() {
    k9::assert_equal!(run_one("return math.acos(1)"), Value::Float(0.0));
}

#[test]
fn math_acos_zero() {
    k9::assert_equal!(run_one("return math.acos(0)"), Value::Float(0.0_f64.acos()));
}

#[test]
fn math_atan_zero() {
    k9::assert_equal!(run_one("return math.atan(0)"), Value::Float(0.0));
}

#[test]
fn math_atan_one() {
    k9::assert_equal!(run_one("return math.atan(1)"), Value::Float(1.0_f64.atan()));
}

#[test]
fn math_atan_two_args() {
    // atan(1, 1) == atan2(1, 1) == pi/4
    k9::assert_equal!(
        run_one("return math.atan(1, 1)"),
        Value::Float(1.0_f64.atan2(1.0))
    );
}

#[test]
fn math_atan_two_args_negative() {
    // atan(-1, -1) == atan2(-1, -1) == -3*pi/4
    k9::assert_equal!(
        run_one("return math.atan(-1, -1)"),
        Value::Float((-1.0_f64).atan2(-1.0))
    );
}

#[test]
fn math_atan_bad_second_arg() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.atan, 1, 'x') return ok"),
        Value::Boolean(false)
    );
}

#[test]
fn math_trig_roundtrip() {
    // asin(sin(x)) should return x for x in [-pi/2, pi/2].
    k9::assert_equal!(
        run_one("return math.asin(math.sin(0.5)) - 0.5 < 1e-10"),
        Value::Boolean(true)
    );
}

// ---------------------------------------------------------------------------
// math.min
// ---------------------------------------------------------------------------

#[test]
fn math_min_two() {
    k9::assert_equal!(run_one("return math.min(3, 1)"), Value::Integer(1));
}

#[test]
fn math_min_many() {
    k9::assert_equal!(run_one("return math.min(5, 3, 8, 1, 4)"), Value::Integer(1));
}

#[test]
fn math_min_single() {
    k9::assert_equal!(run_one("return math.min(42)"), Value::Integer(42));
}

#[test]
fn math_min_negative() {
    k9::assert_equal!(
        run_one("return math.min(-10, -20, -5)"),
        Value::Integer(-20)
    );
}

#[test]
fn math_min_mixed_int_float() {
    // Should return the float since 1.5 < 2.
    k9::assert_equal!(run_one("return math.min(2, 1.5)"), Value::Float(1.5));
}

#[test]
fn math_min_preserves_integer() {
    // When the minimum is an integer, it stays integer.
    k9::assert_equal!(run_one("return math.min(1, 2.5)"), Value::Integer(1));
}

#[test]
fn math_min_no_args() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.min) return ok"),
        Value::Boolean(false)
    );
}

#[test]
fn math_min_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.min, 1, 'x') return ok"),
        Value::Boolean(false)
    );
}

#[test]
fn math_min_tie_returns_first() {
    // Equal values: the first argument wins, preserving its type.
    k9::assert_equal!(run_one("return math.min(1, 1.0)"), Value::Integer(1));
}

// ---------------------------------------------------------------------------
// math.max
// ---------------------------------------------------------------------------

#[test]
fn math_max_two() {
    k9::assert_equal!(run_one("return math.max(3, 1)"), Value::Integer(3));
}

#[test]
fn math_max_many() {
    k9::assert_equal!(run_one("return math.max(5, 3, 8, 1, 4)"), Value::Integer(8));
}

#[test]
fn math_max_single() {
    k9::assert_equal!(run_one("return math.max(42)"), Value::Integer(42));
}

#[test]
fn math_max_negative() {
    k9::assert_equal!(run_one("return math.max(-10, -20, -5)"), Value::Integer(-5));
}

#[test]
fn math_max_mixed_int_float() {
    // Should return the float since 3.5 > 2.
    k9::assert_equal!(run_one("return math.max(2, 3.5)"), Value::Float(3.5));
}

#[test]
fn math_max_preserves_integer() {
    k9::assert_equal!(run_one("return math.max(3, 1.5)"), Value::Integer(3));
}

#[test]
fn math_max_no_args() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.max) return ok"),
        Value::Boolean(false)
    );
}

#[test]
fn math_max_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.max, 1, true) return ok"),
        Value::Boolean(false)
    );
}

#[test]
fn math_max_tie_returns_first() {
    // Equal values: the first argument wins, preserving its type.
    k9::assert_equal!(run_one("return math.max(1, 1.0)"), Value::Integer(1));
}

// ---------------------------------------------------------------------------
// math.tointeger
// ---------------------------------------------------------------------------

#[test]
fn math_tointeger_from_integer() {
    k9::assert_equal!(run_one("return math.tointeger(42)"), Value::Integer(42));
}

#[test]
fn math_tointeger_from_whole_float() {
    k9::assert_equal!(run_one("return math.tointeger(5.0)"), Value::Integer(5));
}

#[test]
fn math_tointeger_fractional_fails() {
    k9::assert_equal!(run_one("return math.tointeger(3.14)"), Value::Nil);
}

#[test]
fn math_tointeger_infinity_fails() {
    k9::assert_equal!(run_one("return math.tointeger(math.huge)"), Value::Nil);
}

#[test]
fn math_tointeger_nan_fails() {
    k9::assert_equal!(run_one("return math.tointeger(0/0)"), Value::Nil);
}

#[test]
fn math_tointeger_string_fails() {
    k9::assert_equal!(run_one("return math.tointeger('42')"), Value::Nil);
}

#[test]
fn math_tointeger_nil_fails() {
    k9::assert_equal!(run_one("return math.tointeger(nil)"), Value::Nil);
}

#[test]
fn math_tointeger_negative() {
    k9::assert_equal!(run_one("return math.tointeger(-7.0)"), Value::Integer(-7));
}

// ---------------------------------------------------------------------------
// math.type
// ---------------------------------------------------------------------------

#[test]
fn math_type_integer() {
    k9::assert_equal!(run_one("return math.type(42)"), Value::string("integer"));
}

#[test]
fn math_type_float() {
    k9::assert_equal!(run_one("return math.type(3.14)"), Value::string("float"));
}

#[test]
fn math_type_string() {
    k9::assert_equal!(run_one("return math.type('hello')"), Value::Boolean(false));
}

#[test]
fn math_type_nil() {
    k9::assert_equal!(run_one("return math.type(nil)"), Value::Boolean(false));
}

#[test]
fn math_type_boolean() {
    k9::assert_equal!(run_one("return math.type(true)"), Value::Boolean(false));
}

#[test]
fn math_type_integer_zero() {
    // 0 is an integer, not a float.
    k9::assert_equal!(run_one("return math.type(0)"), Value::string("integer"));
}

#[test]
fn math_type_float_zero() {
    k9::assert_equal!(run_one("return math.type(0.0)"), Value::string("float"));
}

// ---------------------------------------------------------------------------
// math.random
// ---------------------------------------------------------------------------

#[test]
fn math_random_no_args_returns_float() {
    // No args: returns a float in [0, 1).
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed(42)
            local r = math.random()
            return type(r) == 'number' and r >= 0 and r < 1"
        ),
        Value::Boolean(true)
    );
}

#[test]
fn math_random_no_args_is_float_type() {
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed(42)
            return math.type(math.random())"
        ),
        Value::string("float")
    );
}

#[test]
fn math_random_one_arg() {
    // One arg m: returns an integer in [1, m].
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed(42)
            local r = math.random(10)
            return math.type(r) == 'integer' and r >= 1 and r <= 10"
        ),
        Value::Boolean(true)
    );
}

#[test]
fn math_random_two_args() {
    // Two args: returns an integer in [m, n].
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed(42)
            local r = math.random(5, 10)
            return math.type(r) == 'integer' and r >= 5 and r <= 10"
        ),
        Value::Boolean(true)
    );
}

#[test]
fn math_random_one_arg_is_one() {
    // math.random(1) always returns 1.
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed(42)
            return math.random(1)"
        ),
        Value::Integer(1)
    );
}

#[test]
fn math_random_same_bounds() {
    // math.random(5, 5) always returns 5.
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed(42)
            return math.random(5, 5)"
        ),
        Value::Integer(5)
    );
}

#[test]
fn math_random_empty_interval_one_arg() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.random, 0) return ok"),
        Value::Boolean(false)
    );
}

#[test]
fn math_random_empty_interval_two_args() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.random, 10, 5) return ok"),
        Value::Boolean(false)
    );
}

#[test]
fn math_random_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.random, 'x') return ok"),
        Value::Boolean(false)
    );
}

// ---------------------------------------------------------------------------
// math.randomseed
// ---------------------------------------------------------------------------

#[test]
fn math_randomseed_deterministic() {
    // Same seed produces the same sequence.
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed(123)
            local a = math.random()
            math.randomseed(123)
            local b = math.random()
            return a == b"
        ),
        Value::Boolean(true)
    );
}

#[test]
fn math_randomseed_no_args() {
    // No args uses a time-based seed; just check it doesn't error.
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed()
            local r = math.random()
            return r >= 0 and r < 1"
        ),
        Value::Boolean(true)
    );
}

#[test]
fn math_randomseed_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.randomseed, 'x') return ok"),
        Value::Boolean(false)
    );
}

#[test]
fn math_random_negative_one_arg() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.random, -1) return ok"),
        Value::Boolean(false)
    );
}

#[test]
fn math_random_float_coercion() {
    // 10.5 truncates to 10; result should be in [1, 10].
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed(42)
            local r = math.random(10.5)
            return r >= 1 and r <= 10"
        ),
        Value::Boolean(true)
    );
}

#[test]
fn math_random_sequence_varies() {
    // Multiple calls should not all return the same value.
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed(42)
            local a = math.random()
            local b = math.random()
            local c = math.random()
            return a ~= b or b ~= c"
        ),
        Value::Boolean(true)
    );
}

#[test]
fn math_randomseed_different_seeds_diverge() {
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed(1)
            local a = math.random()
            math.randomseed(2)
            local b = math.random()
            return a ~= b"
        ),
        Value::Boolean(true)
    );
}
