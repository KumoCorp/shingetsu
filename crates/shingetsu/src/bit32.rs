//! Implementation of the `bit32` standard library module.
//!
//! Provides bitwise operations on unsigned 32-bit integers, matching
//! the Luau `bit32` API.  All functions convert their arguments to
//! unsigned 32-bit integers before operating: integer values are
//! masked to their low 32 bits, and float values are rounded to the
//! nearest integer, then masked.

use crate::convert::{Number, TypedVariadic};
use crate::value::Value;
use crate::VmError;

// ---------------------------------------------------------------------------
// BitU32 — coerced u32 newtype
// ---------------------------------------------------------------------------

/// An unsigned 32-bit integer extracted from a Lua number.
///
/// This is the argument type for `bit32` functions.  Lua integer
/// values are masked to their low 32 bits (so `-1` becomes
/// `0xFFFFFFFF`); Lua float values are rounded to the nearest
/// integer, then masked.  Non-number values, NaN, and infinity
/// produce a `BadArgument` error.
#[derive(Clone, Copy, Debug)]
struct BitU32(u32);

impl crate::FromLua for BitU32 {
    fn from_lua(v: Value) -> Result<Self, VmError> {
        let n = Number::from_lua(v)?;
        Ok(BitU32(match n {
            Number::Integer(i) => i as u32,
            Number::Float(f) => {
                if !f.is_finite() {
                    return Err(VmError::ArgError {
                        position: 0,
                        function: String::new(),
                        msg: "number has no integer representation".to_owned(),
                    });
                }
                (f.round() as i64) as u32
            }
        }))
    }
}

impl crate::IntoLua for BitU32 {
    fn into_lua(self) -> Value {
        Value::Integer(self.0 as i64)
    }
}

impl crate::LuaTyped for BitU32 {
    fn lua_type() -> crate::types::LuaType {
        crate::types::LuaType::Number
    }
}

// ---------------------------------------------------------------------------
// Shift / rotate helpers (following Luau lbitlib.cpp semantics)
// ---------------------------------------------------------------------------

/// Logical left shift.  Negative displacements shift right.
///
/// Uses [`i64::unsigned_abs`] for the negation so a `disp` of
/// `i64::MIN` does not overflow.
fn shift_left(r: u32, disp: i64) -> u32 {
    if disp < 0 {
        let d = disp.unsigned_abs();
        if d >= 32 {
            0
        } else {
            r >> (d as u32)
        }
    } else if disp >= 32 {
        0
    } else {
        r << (disp as u32)
    }
}

/// Logical right shift.  Negative displacements shift left.
fn shift_right(r: u32, disp: i64) -> u32 {
    if disp < 0 {
        let d = disp.unsigned_abs();
        if d >= 32 {
            0
        } else {
            r << (d as u32)
        }
    } else if disp >= 32 {
        0
    } else {
        r >> (disp as u32)
    }
}

/// Arithmetic right shift.  Fills vacant bits with copies of bit 31.
///
/// Implemented via signed `i32` shift so the sign extension is the
/// hardware's native behaviour rather than a hand-rolled mask
/// (which mishandles `disp == 0`).
fn shift_arith_right(r: u32, disp: i64) -> u32 {
    if disp < 0 {
        let d = disp.unsigned_abs();
        if d >= 32 {
            0
        } else {
            r << (d as u32)
        }
    } else if disp >= 32 {
        if r & 0x80000000 != 0 {
            0xFFFFFFFF
        } else {
            0
        }
    } else {
        ((r as i32) >> (disp as u32)) as u32
    }
}

// ---------------------------------------------------------------------------
// Module registration
// ---------------------------------------------------------------------------

/// Build the bit32 library table and register it as the `bit32`
/// global.  Called by [`register_libs`] when builtins are enabled.
///
/// [`register_libs`]: crate::register_libs
pub fn register(env: &crate::GlobalEnv) -> Result<(), VmError> {
    let table = bit32_mod::build_module_table(env)?;
    env.set_global("bit32", Value::Table(table));
    env.register_module_type("bit32", bit32_mod::module_type());
    Ok(())
}

// ---------------------------------------------------------------------------
// bit32 module
// ---------------------------------------------------------------------------

/// Bitwise operations on unsigned 32-bit integers.
///
/// All functions in this module convert their number arguments to
/// unsigned 32-bit integers before operating.  Integer values are
/// masked to their low 32 bits; float values are rounded to the
/// nearest integer, then masked.  The result is always a non-negative
/// integer in the range `[0, 2^32 - 1]`.
///
/// Negative displacements passed to shift functions reverse the
/// direction: `bit32.lshift(x, -n)` is equivalent to
/// `bit32.rshift(x, n)`, and vice versa.
#[crate::module(name = "bit32")]
pub mod bit32_mod {
    use super::*;

    /// Return the bitwise AND of all arguments.
    ///
    /// Each bit of the result is `1` only if every argument has a `1`
    /// in that position.  With no arguments the result is all ones
    /// (`0xFFFFFFFF`), which is the identity value for AND.
    ///
    /// # Parameters
    ///
    /// - `...` — zero or more numbers to AND together
    ///
    /// # Returns
    ///
    /// - the bitwise AND of all arguments, or `0xFFFFFFFF` when called
    ///   with no arguments
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(bit32.band(1, 3) == 1)
    /// assert(bit32.band(0xFF, 0x0F) == 0x0F)
    /// assert(bit32.band() == 0xFFFFFFFF)
    /// assert(bit32.band(0xA, 0xC, 0xF) == 0x8)
    /// -- Negative integers are masked to their low 32 bits.
    /// assert(bit32.band(-1, 0xFFFF) == 0xFFFF)
    /// -- Floats are rounded to the nearest integer first.
    /// assert(bit32.band(3.7, 0xFF) == 4)
    /// ```
    #[function(variadic)]
    fn band(args: TypedVariadic<BitU32>) -> BitU32 {
        let mut r = 0xFFFFFFFFu32;
        for BitU32(v) in args.0 {
            r &= v;
        }
        BitU32(r)
    }

    /// Return the bitwise OR of all arguments.
    ///
    /// Each bit of the result is `1` if any argument has a `1` in that
    /// position.  With no arguments the result is zero, which is the
    /// identity value for OR.
    ///
    /// # Parameters
    ///
    /// - `...` — zero or more numbers to OR together
    ///
    /// # Returns
    ///
    /// - the bitwise OR of all arguments, or `0` when called with no
    ///   arguments
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(bit32.bor(1, 2) == 3)
    /// assert(bit32.bor(0xF0, 0x0F) == 0xFF)
    /// assert(bit32.bor() == 0)
    /// assert(bit32.bor(0xA, 0x5) == 0xF)
    /// ```
    #[function(variadic)]
    fn bor(args: TypedVariadic<BitU32>) -> BitU32 {
        let mut r = 0u32;
        for BitU32(v) in args.0 {
            r |= v;
        }
        BitU32(r)
    }

    /// Return the bitwise XOR of all arguments.
    ///
    /// Each bit of the result is `1` if an odd number of arguments have
    /// a `1` in that position.  With no arguments the result is zero,
    /// which is the identity value for XOR.
    ///
    /// # Parameters
    ///
    /// - `...` — zero or more numbers to XOR together
    ///
    /// # Returns
    ///
    /// - the bitwise XOR of all arguments, or `0` when called with no
    ///   arguments
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(bit32.bxor(1, 3) == 2)
    /// assert(bit32.bxor(0xFF, 0x0F) == 0xF0)
    /// assert(bit32.bxor() == 0)
    /// assert(bit32.bxor(0xA, 0xA) == 0)
    /// ```
    #[function(variadic)]
    fn bxor(args: TypedVariadic<BitU32>) -> BitU32 {
        let mut r = 0u32;
        for BitU32(v) in args.0 {
            r ^= v;
        }
        BitU32(r)
    }

    /// Return the bitwise NOT of `x`.
    ///
    /// For any integer `x`, the identity `bit32.bnot(x) ==
    /// (-1 - x) % 2^32` holds.
    ///
    /// # Parameters
    ///
    /// - `x` — number to negate
    ///
    /// # Returns
    ///
    /// - the bitwise complement of `x`, masked to 32 bits
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(bit32.bnot(0) == 0xFFFFFFFF)
    /// assert(bit32.bnot(0xFFFFFFFF) == 0)
    /// assert(bit32.bnot(0x0F0F0F0F) == 0xF0F0F0F0)
    /// -- The (-1 - x) % 2^32 identity, exercised through coercion.
    /// assert(bit32.bnot(-1) == 0)
    /// ```
    #[function]
    fn bnot(x: BitU32) -> BitU32 {
        BitU32(!x.0)
    }

    /// Return whether the bitwise AND of all arguments is non-zero.
    ///
    /// Equivalent to `bit32.band(...) ~= 0` but returns a boolean
    /// instead of a number.  With no arguments the AND is all-ones
    /// (the identity element), so `bit32.btest()` returns `true`.
    ///
    /// # Parameters
    ///
    /// - `...` — zero or more numbers to test
    ///
    /// # Returns
    ///
    /// - `true` if the AND of all arguments is non-zero, `false`
    ///   otherwise; `true` when called with no arguments
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(bit32.btest(1, 3) == true)
    /// assert(bit32.btest(1, 2) == false)
    /// assert(bit32.btest(0xFF, 0x0F) == true)
    /// assert(bit32.btest(0x10, 0x01) == false)
    /// assert(bit32.btest() == true)
    /// ```
    #[function(variadic)]
    fn btest(args: TypedVariadic<BitU32>) -> bool {
        let mut r = 0xFFFFFFFFu32;
        for BitU32(v) in args.0 {
            r &= v;
        }
        r != 0
    }

    /// Return `x` with its bits shifted left by `disp` positions.
    ///
    /// This is a logical (zero-fill) left shift.  Vacant bits on the
    /// right are filled with zeros; bits shifted past position 31 are
    /// discarded.
    ///
    /// Negative displacements shift right instead: `lshift(x, -n)`
    /// is equivalent to `rshift(x, n)`.
    ///
    /// # Parameters
    ///
    /// - `x` — the number whose bits are shifted
    /// - `disp` — number of bits to shift; negative shifts right
    ///
    /// # Returns
    ///
    /// - the shifted value, masked to 32 bits
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(bit32.lshift(1, 4) == 16)
    /// assert(bit32.lshift(0xFF, 24) == 0xFF000000)
    /// assert(bit32.lshift(0xFF, 32) == 0)
    /// assert(bit32.lshift(1, -2) == bit32.rshift(1, 2))
    /// ```
    #[function]
    fn lshift(x: BitU32, disp: i64) -> BitU32 {
        BitU32(shift_left(x.0, disp))
    }

    /// Return `x` with its bits shifted right by `disp` positions.
    ///
    /// This is a logical (zero-fill) right shift.  Vacant bits on the
    /// left are filled with zeros; bits shifted past position 0 are
    /// discarded.
    ///
    /// Negative displacements shift left instead: `rshift(x, -n)`
    /// is equivalent to `lshift(x, n)`.
    ///
    /// # Parameters
    ///
    /// - `x` — the number whose bits are shifted
    /// - `disp` — number of bits to shift; negative shifts left
    ///
    /// # Returns
    ///
    /// - the shifted value, masked to 32 bits
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(bit32.rshift(16, 4) == 1)
    /// assert(bit32.rshift(0xFF000000, 24) == 0xFF)
    /// assert(bit32.rshift(1, 32) == 0)
    /// assert(bit32.rshift(4, -2) == bit32.lshift(4, 2))
    /// ```
    #[function]
    fn rshift(x: BitU32, disp: i64) -> BitU32 {
        BitU32(shift_right(x.0, disp))
    }

    /// Return `x` with its bits arithmetically shifted right by
    /// `disp` positions.
    ///
    /// This is an arithmetic right shift: vacant bits on the left are
    /// filled with copies of bit 31 (the sign bit).  When `x` has
    /// bit 31 clear, this behaves the same as `bit32.rshift`.  When
    /// bit 31 is set, the result sign-extends into the vacated
    /// positions.
    ///
    /// Negative displacements shift left instead: `arshift(x, -n)`
    /// is equivalent to `lshift(x, n)`.
    ///
    /// # Parameters
    ///
    /// - `x` — the number whose bits are shifted
    /// - `disp` — number of bits to shift; negative shifts left
    ///
    /// # Returns
    ///
    /// - the arithmetically shifted value, masked to 32 bits
    ///
    /// # Examples
    ///
    /// ```lua
    /// -- Positive number: same as rshift.
    /// assert(bit32.arshift(16, 4) == 1)
    /// assert(bit32.arshift(0x7FFFFFFF, 32) == 0)
    /// -- Negative number: sign-extends from bit 31.
    /// assert(bit32.arshift(0xFFFFFFFF, 1) == 0xFFFFFFFF)
    /// assert(bit32.arshift(0x80000000, 4) == 0xF8000000)
    /// assert(bit32.arshift(0x80000000, 32) == 0xFFFFFFFF)
    /// -- Zero displacement is the identity, even with bit 31 set.
    /// assert(bit32.arshift(0xFFFFFFFF, 0) == 0xFFFFFFFF)
    /// ```
    #[function]
    fn arshift(x: BitU32, disp: i64) -> BitU32 {
        BitU32(shift_arith_right(x.0, disp))
    }

    /// Return `x` with its bits rotated left by `disp` positions.
    ///
    /// Bits that rotate past position 31 wrap around to position 0.
    /// Negative displacements rotate right instead: `lrotate(x, -n)`
    /// is equivalent to `rrotate(x, n)`.
    ///
    /// The displacement is taken modulo 32, so `lrotate(x, 33)` is
    /// the same as `lrotate(x, 1)`.
    ///
    /// # Parameters
    ///
    /// - `x` — the number whose bits are rotated
    /// - `disp` — number of bits to rotate; negative rotates right
    ///
    /// # Returns
    ///
    /// - the rotated value, masked to 32 bits
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(bit32.lrotate(1, 4) == 16)
    /// assert(bit32.lrotate(1, 32) == 1)
    /// assert(bit32.lrotate(1, -4) == bit32.rrotate(1, 4))
    /// assert(bit32.lrotate(0x80000000, 1) == 1)
    /// ```
    #[function]
    fn lrotate(x: BitU32, disp: i64) -> BitU32 {
        let r = x.0;
        // disp.rem_euclid(32) on i64 always returns a value in [0, 31],
        // so this is safe even for disp == i64::MIN.
        let d = disp.rem_euclid(32) as u32;
        if d == 0 {
            BitU32(r)
        } else {
            BitU32((r << d) | (r >> (32 - d)))
        }
    }

    /// Return `x` with its bits rotated right by `disp` positions.
    ///
    /// Bits that rotate past position 0 wrap around to position 31.
    /// Negative displacements rotate left instead: `rrotate(x, -n)`
    /// is equivalent to `lrotate(x, n)`.
    ///
    /// The displacement is taken modulo 32, so `rrotate(x, 33)` is
    /// the same as `rrotate(x, 1)`.
    ///
    /// # Parameters
    ///
    /// - `x` — the number whose bits are rotated
    /// - `disp` — number of bits to rotate; negative rotates left
    ///
    /// # Returns
    ///
    /// - the rotated value, masked to 32 bits
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(bit32.rrotate(16, 4) == 1)
    /// assert(bit32.rrotate(1, 32) == 1)
    /// assert(bit32.rrotate(1, -4) == bit32.lrotate(1, 4))
    /// assert(bit32.rrotate(0x01, 1) == 0x80000000)
    /// ```
    #[function]
    fn rrotate(x: BitU32, disp: i64) -> BitU32 {
        let r = x.0;
        // disp.rem_euclid(32) on i64 always returns a value in [0, 31],
        // so this is safe even for disp == i64::MIN.
        let d = disp.rem_euclid(32) as u32;
        if d == 0 {
            BitU32(r)
        } else {
            BitU32((r >> d) | (r << (32 - d)))
        }
    }

    /// Extract a range of bits from `n` and return them as an
    /// unsigned number.
    ///
    /// Returns the `width` bits of `n` starting at bit position
    /// `field`, where bit 0 is the least significant bit.  The default
    /// `width` is 1.
    ///
    /// # Parameters
    ///
    /// - `n` — the number to extract bits from
    /// - `field` — 0-based starting bit position (0 = least significant)
    /// - `width` — number of bits to extract; defaults to 1
    ///
    /// # Returns
    ///
    /// - the extracted bits as an unsigned number
    ///
    /// # Errors
    ///
    /// Raises an error when `field < 0`, `width < 1`, or
    /// `field + width > 32`.
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(bit32.extract(0xABCD, 0, 4) == 0xD)
    /// assert(bit32.extract(0xABCD, 4, 8) == 0xBC)
    /// assert(bit32.extract(0xABCD, 12, 4) == 0xA)
    /// assert(bit32.extract(0xFF, 0) == 1)
    /// ```
    #[function]
    fn extract(n: BitU32, field: i64, width: Option<i64>) -> Result<BitU32, VmError> {
        let f = field;
        let w = width.unwrap_or(1);
        if f < 0 {
            return Err(VmError::ArgError {
                position: 2,
                function: "extract".to_owned(),
                msg: "field cannot be negative".to_owned(),
            });
        }
        if w < 1 {
            return Err(VmError::ArgError {
                position: 3,
                function: "extract".to_owned(),
                msg: "width must be positive".to_owned(),
            });
        }
        if f + w > 32 {
            return Err(VmError::ArgError {
                position: 2,
                function: "extract".to_owned(),
                msg: "trying to access non-existent bits".to_owned(),
            });
        }
        let mask = if w >= 32 {
            0xFFFFFFFFu32
        } else {
            (1u32 << w) - 1
        };
        Ok(BitU32((n.0 >> f as u32) & mask))
    }

    /// Return a copy of `n` with a range of bits starting at
    /// position `field` replaced by the low `width` bits of `v`.
    ///
    /// The default `width` is 1.  Bits of `v` outside the specified
    /// width are ignored.
    ///
    /// # Parameters
    ///
    /// - `n` — the number whose bits are replaced
    /// - `v` — the replacement value
    /// - `field` — 0-based starting bit position (0 = least significant)
    /// - `width` — number of bits to replace; defaults to 1
    ///
    /// # Returns
    ///
    /// - `n` with the specified bit range replaced by `v`
    ///
    /// # Errors
    ///
    /// Raises an error when `field < 0`, `width < 1`, or
    /// `field + width > 32`.
    ///
    /// # Examples
    ///
    /// ```lua
    /// -- Replace 4 bits at position 0.
    /// assert(bit32.replace(0x00000000, 0xD, 0, 4) == 0xD)
    /// -- Replace 8 bits at position 4.
    /// assert(bit32.replace(0x0000AB00, 0xCD, 4, 8) == 0x0000ACD0)
    /// -- Only the low 4 bits of v are used when width is 4.
    /// assert(bit32.replace(0, 0xFF, 0, 4) == 0xF)
    /// ```
    #[function]
    fn replace(n: BitU32, v: BitU32, field: i64, width: Option<i64>) -> Result<BitU32, VmError> {
        let f = field;
        let w = width.unwrap_or(1);
        if f < 0 {
            return Err(VmError::ArgError {
                position: 3,
                function: "replace".to_owned(),
                msg: "field cannot be negative".to_owned(),
            });
        }
        if w < 1 {
            return Err(VmError::ArgError {
                position: 4,
                function: "replace".to_owned(),
                msg: "width must be positive".to_owned(),
            });
        }
        if f + w > 32 {
            return Err(VmError::ArgError {
                position: 3,
                function: "replace".to_owned(),
                msg: "trying to access non-existent bits".to_owned(),
            });
        }
        let mask = if w >= 32 {
            0xFFFFFFFFu32
        } else {
            (1u32 << w) - 1
        };
        let v_masked = v.0 & mask;
        Ok(BitU32((n.0 & !(mask << f as u32)) | (v_masked << f as u32)))
    }

    /// Return the number of consecutive zero bits starting from the
    /// left-most (most significant) bit of `n`.
    ///
    /// Returns 32 when `n` is zero (all bits are zero).
    ///
    /// # Parameters
    ///
    /// - `n` — the number to inspect
    ///
    /// # Returns
    ///
    /// - the count of leading zero bits (0–32)
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(bit32.countlz(0) == 32)
    /// assert(bit32.countlz(1) == 31)
    /// assert(bit32.countlz(0x80000000) == 0)
    /// assert(bit32.countlz(0xFF) == 24)
    /// ```
    #[function]
    fn countlz(n: BitU32) -> i64 {
        i64::from(n.0.leading_zeros())
    }

    /// Return the number of consecutive zero bits starting from the
    /// right-most (least significant) bit of `n`.
    ///
    /// Returns 32 when `n` is zero (all bits are zero).
    ///
    /// # Parameters
    ///
    /// - `n` — the number to inspect
    ///
    /// # Returns
    ///
    /// - the count of trailing zero bits (0–32)
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(bit32.countrz(0) == 32)
    /// assert(bit32.countrz(1) == 0)
    /// assert(bit32.countrz(0x80000000) == 31)
    /// assert(bit32.countrz(0x100) == 8)
    /// ```
    #[function]
    fn countrz(n: BitU32) -> i64 {
        i64::from(n.0.trailing_zeros())
    }

    /// Return `x` with the order of its bytes reversed.
    ///
    /// Byte 0 becomes byte 3, byte 1 becomes byte 2, byte 2 becomes
    /// byte 1, and byte 3 becomes byte 0.
    ///
    /// # Parameters
    ///
    /// - `x` — the number whose bytes are swapped
    ///
    /// # Returns
    ///
    /// - `x` with its bytes in reverse order
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(bit32.byteswap(0x12345678) == 0x78563412)
    /// assert(bit32.byteswap(0xFF000000) == 0x000000FF)
    /// assert(bit32.byteswap(0) == 0)
    /// ```
    #[function]
    fn byteswap(x: BitU32) -> BitU32 {
        let n = x.0;
        BitU32((n << 24) | ((n << 8) & 0x00FF0000) | ((n >> 8) & 0x0000FF00) | (n >> 24))
    }
}
