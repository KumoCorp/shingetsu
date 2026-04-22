//! `string.pack` / `string.unpack` / `string.packsize` implementation.
//!
//! Implements Lua 5.4 §6.4.2 pack format strings for binary data
//! serialization and deserialization.

use bytes::buf::{Buf, BufMut};
use shingetsu::Bytes;

use crate::error::VmError;
use crate::value::Value;

// =========================================================================
// Native sizes — derived from the C types on this platform.
// =========================================================================

/// Native `short` size.
const NATIVE_SHORT: usize = std::mem::size_of::<std::os::raw::c_short>();
/// Native `long` size.
const NATIVE_LONG: usize = std::mem::size_of::<std::os::raw::c_long>();
/// Native `int` size (default for `i`/`I` without explicit size).
const NATIVE_INT: usize = std::mem::size_of::<std::os::raw::c_int>();
/// Native `size_t` size.
const NATIVE_SIZE_T: usize = std::mem::size_of::<usize>();
/// Native `float` size.
const NATIVE_FLOAT: usize = std::mem::size_of::<std::os::raw::c_float>();
/// Native `double` size.
const NATIVE_DOUBLE: usize = std::mem::size_of::<std::os::raw::c_double>();
/// Native alignment (maximum alignment used by `!` without explicit size).
const NATIVE_ALIGNMENT: usize = std::mem::align_of::<std::os::raw::c_longlong>();
/// Maximum allowed integer size for `i[n]`/`I[n]`.
const MAX_INT_SIZE: usize = 16;
/// `lua_Integer` = i64.
const LUA_INTEGER_SIZE: usize = std::mem::size_of::<i64>();
/// `lua_Number` = f64.
const LUA_NUMBER_SIZE: usize = 8;

// =========================================================================
// Endianness
// =========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Endian {
    Little,
    Big,
}

impl Endian {
    fn native() -> Self {
        if cfg!(target_endian = "little") {
            Endian::Little
        } else {
            Endian::Big
        }
    }
}

// =========================================================================
// Format option
// =========================================================================

/// A single parsed format option.
#[derive(Debug, Clone)]
enum FmtOpt {
    /// Signed integer of `size` bytes.
    Int { size: usize },
    /// Unsigned integer of `size` bytes.
    Uint { size: usize },
    /// 32-bit float.
    Float,
    /// 64-bit double.
    Double,
    /// lua_Number (f64).
    LuaNumber,
    /// Fixed-size string of `n` bytes.
    FixedStr { n: usize },
    /// Zero-terminated string.
    ZStr,
    /// Length-prefixed string, length is `len_size`-byte unsigned int.
    LenStr { len_size: usize },
    /// One byte of padding.
    Padding,
    /// Alignment-only item: aligns to the natural alignment of `inner`.
    AlignOnly { align_size: usize },
}

impl FmtOpt {
    /// The natural size of this option (used for alignment calculations).
    fn size(&self) -> usize {
        match self {
            FmtOpt::Int { size } | FmtOpt::Uint { size } => *size,
            FmtOpt::Float => NATIVE_FLOAT,
            FmtOpt::Double => NATIVE_DOUBLE,
            FmtOpt::LuaNumber => LUA_NUMBER_SIZE,
            FmtOpt::FixedStr { .. } | FmtOpt::ZStr => 1,
            FmtOpt::LenStr { len_size } => *len_size,
            FmtOpt::Padding => 1,
            FmtOpt::AlignOnly { align_size } => *align_size,
        }
    }

    /// Whether this option is variable-length (disallowed in packsize).
    fn is_variable_length(&self) -> bool {
        matches!(self, FmtOpt::ZStr | FmtOpt::LenStr { .. })
    }
}

// =========================================================================
// Format parser
// =========================================================================

/// State accumulated while parsing a format string.
struct FmtParser<'a> {
    fmt: &'a [u8],
    pos: usize,
    endian: Endian,
    max_align: usize,
    /// Name of the public function ("pack", "unpack", "packsize") used
    /// for error wrapping — errors discovered during format parsing
    /// surface as `bad argument #1 to '{func_name}' (...)`.
    func_name: &'static str,
}

impl<'a> FmtParser<'a> {
    fn new(fmt: &'a [u8], func_name: &'static str) -> Self {
        Self {
            fmt,
            pos: 0,
            // Default: "!1=" — max alignment 1, native endian.
            endian: Endian::native(),
            max_align: 1,
            func_name,
        }
    }

    /// Read an optional integer immediately following the current char.
    /// Returns `None` if no digits follow.
    fn read_optional_int(&mut self) -> Result<Option<usize>, VmError> {
        let start = self.pos;
        while self.pos < self.fmt.len() && self.fmt[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        if self.pos == start {
            return Ok(None);
        }
        let s = std::str::from_utf8(&self.fmt[start..self.pos]).expect("ascii digits");
        match s.parse::<usize>() {
            Ok(n) => Ok(Some(n)),
            Err(_) => Err(pack_error(format!(
                "invalid format (size '{}' too large)",
                s
            ))),
        }
    }

    /// Read a required integer (for `c<n>`).
    fn read_required_int(&mut self, opt_char: char) -> Result<usize, VmError> {
        self.read_optional_int()?
            .ok_or_else(|| pack_error(format!("missing size for format option '{}'", opt_char)))
    }

    /// Parse the next format option, updating endianness/alignment state.
    /// Returns `None` at end of format string.
    fn next_opt(&mut self) -> Result<Option<FmtOpt>, VmError> {
        loop {
            if self.pos >= self.fmt.len() {
                return Ok(None);
            }
            let ch = self.fmt[self.pos];
            self.pos += 1;
            match ch {
                // Endian and alignment config — these are not data options,
                // so we loop to parse the next actual option.
                b'<' => {
                    self.endian = Endian::Little;
                    continue;
                }
                b'>' => {
                    self.endian = Endian::Big;
                    continue;
                }
                b'=' => {
                    self.endian = Endian::native();
                    continue;
                }
                b'!' => {
                    let n = self.read_optional_int()?.unwrap_or(NATIVE_ALIGNMENT);
                    // Lua validates the size bounds at parse time but defers
                    // the power-of-two check until the alignment is actually
                    // applied to a data option.
                    validate_int_size(n, '!')?;
                    self.max_align = n;
                    continue;
                }
                b' ' => continue,

                // Signed integers
                b'b' => return Ok(Some(FmtOpt::Int { size: 1 })),
                b'h' => return Ok(Some(FmtOpt::Int { size: NATIVE_SHORT })),
                b'l' => return Ok(Some(FmtOpt::Int { size: NATIVE_LONG })),
                b'j' => {
                    return Ok(Some(FmtOpt::Int {
                        size: LUA_INTEGER_SIZE,
                    }))
                }
                b'i' => {
                    let n = self.read_optional_int()?.unwrap_or(NATIVE_INT);
                    validate_int_size(n, 'i')?;
                    return Ok(Some(FmtOpt::Int { size: n }));
                }

                // Unsigned integers
                b'B' => return Ok(Some(FmtOpt::Uint { size: 1 })),
                b'H' => return Ok(Some(FmtOpt::Uint { size: NATIVE_SHORT })),
                b'L' => return Ok(Some(FmtOpt::Uint { size: NATIVE_LONG })),
                b'J' => {
                    return Ok(Some(FmtOpt::Uint {
                        size: LUA_INTEGER_SIZE,
                    }))
                }
                b'T' => {
                    return Ok(Some(FmtOpt::Uint {
                        size: NATIVE_SIZE_T,
                    }))
                }
                b'I' => {
                    let n = self.read_optional_int()?.unwrap_or(NATIVE_INT);
                    validate_int_size(n, 'I')?;
                    return Ok(Some(FmtOpt::Uint { size: n }));
                }

                // Floats
                b'f' => return Ok(Some(FmtOpt::Float)),
                b'd' => return Ok(Some(FmtOpt::Double)),
                b'n' => return Ok(Some(FmtOpt::LuaNumber)),

                // Strings
                b'c' => {
                    let n = self.read_required_int('c')?;
                    return Ok(Some(FmtOpt::FixedStr { n }));
                }
                b'z' => return Ok(Some(FmtOpt::ZStr)),
                b's' => {
                    let n = self.read_optional_int()?.unwrap_or(NATIVE_SIZE_T);
                    validate_int_size(n, 's')?;
                    return Ok(Some(FmtOpt::LenStr { len_size: n }));
                }

                // Padding & alignment
                b'x' => return Ok(Some(FmtOpt::Padding)),
                b'X' => {
                    // Xop — peek the next option to get its alignment, but
                    // don't consume a value. Lua calls `getoption` exactly
                    // once for X's follower, so any byte that `getoption`
                    // classifies as `Knop` (space, endian `<`/`>`/`=`,
                    // alignment `!`) or as another `X` is rejected here,
                    // even though the outer parser loop would otherwise
                    // skip past such bytes transparently.
                    match self.fmt.get(self.pos).copied() {
                        None | Some(b' ' | b'<' | b'>' | b'=' | b'!' | b'X') => {
                            return Err(arg_error(
                                self.func_name,
                                ArgPos::Format,
                                "invalid next option for option 'X'",
                            ));
                        }
                        _ => {}
                    }
                    let inner = self.next_opt()?.ok_or_else(|| {
                        arg_error(
                            self.func_name,
                            ArgPos::Format,
                            "invalid next option for option 'X'",
                        )
                    })?;
                    // `c<n>` and `z` have no well-defined alignment, so
                    // reject those too. (`AlignOnly` can't reach here
                    // because its only source is another `X`, already
                    // rejected above.)
                    if matches!(inner, FmtOpt::FixedStr { .. } | FmtOpt::ZStr) {
                        return Err(arg_error(
                            self.func_name,
                            ArgPos::Format,
                            "invalid next option for option 'X'",
                        ));
                    }
                    return Ok(Some(FmtOpt::AlignOnly {
                        align_size: inner.size(),
                    }));
                }

                _ => {
                    return Err(pack_error(format!(
                        "invalid format option '{}'",
                        ch as char
                    )));
                }
            }
        }
    }

    /// Compute alignment padding for `opt` at offset `offset`.
    ///
    /// Defers the power-of-two check on `max_align` to here: Lua accepts
    /// e.g. `!3` in the format string and only errors if an option actually
    /// requires that alignment to be applied.
    fn align_padding(&self, opt: &FmtOpt, offset: usize) -> Result<usize, VmError> {
        let opt_size = opt.size();
        if opt_size == 0 {
            return Ok(0);
        }
        // Alignment is min(option_size, max_alignment).
        let align = std::cmp::min(opt_size, self.max_align);
        if align <= 1 {
            return Ok(0);
        }
        if !align.is_power_of_two() {
            return Err(arg_error(
                self.func_name,
                ArgPos::Format,
                "format asks for alignment not power of 2",
            ));
        }
        // Round up to next multiple of align.
        let remainder = offset % align;
        Ok(if remainder == 0 { 0 } else { align - remainder })
    }
}

fn validate_int_size(n: usize, _opt: char) -> Result<(), VmError> {
    if n == 0 || n > MAX_INT_SIZE {
        return Err(pack_error(format!(
            "integral size ({}) out of limits [1,{}]",
            n, MAX_INT_SIZE
        )));
    }
    Ok(())
}

fn pack_error(msg: impl Into<String>) -> VmError {
    let msg = msg.into();
    // Lua errors raised from pack/unpack/packsize surface to `pcall` as
    // the error message string, not as `nil`.
    let value = Value::string(msg.clone().into_bytes());
    VmError::LuaError {
        display: msg,
        value,
    }
}

/// Position of an argument being reported on for `ArgError`.
///
/// Lua's `string.pack` / `string.unpack` / `string.packsize` wrap
/// value-related errors with `bad argument #N to 'F' (msg)` where `N`
/// refers to the argument that caused the error.
#[derive(Clone, Copy)]
enum ArgPos {
    /// The format string argument — always position 1.
    Format,
    /// A packed value at index `i` (0-based): position `i + 2`
    /// (skipping the format-string arg).
    Value(usize),
    /// The data string argument to `string.unpack` — position 2.
    Data,
    /// The optional init position argument to `string.unpack` — position 3.
    InitPos,
}

impl ArgPos {
    fn to_position(self) -> usize {
        match self {
            ArgPos::Format => 1,
            ArgPos::Value(i) => i + 2,
            ArgPos::Data => 2,
            ArgPos::InitPos => 3,
        }
    }
}

/// Build an `ArgError` for the given function and argument position.
fn arg_error(func: &str, pos: ArgPos, msg: impl Into<String>) -> VmError {
    VmError::ArgError {
        position: pos.to_position(),
        function: func.to_owned(),
        msg: msg.into(),
    }
}

// =========================================================================
// Integer encoding / decoding helpers
//
// For sizes 1–8 we use `BufMut::put_int` / `Buf::get_int` (variable
// width).  For sizes 9–16 we widen to i128/u128 and use the
// `put_i128` / `get_i128` family, slicing to the requested width.
// =========================================================================

/// Returns `true` if signed `val` fits in `size` bytes.
fn signed_fits(val: i64, size: usize) -> bool {
    if size >= 16 {
        return true;
    }
    let wide = val as i128;
    let min = -(1i128 << (size * 8 - 1));
    let max = (1i128 << (size * 8 - 1)) - 1;
    wide >= min && wide <= max
}

/// Returns `true` if unsigned `val` (interpreted as u64) fits in `size` bytes.
fn unsigned_fits(val: i64, size: usize) -> bool {
    if size >= 8 {
        return true;
    }
    let uval = val as u64;
    let max = (1u64 << (size * 8)) - 1;
    uval <= max
}

/// Encode a signed integer `val` into `size` bytes with the given endianness.
///
/// Callers are responsible for range-checking via [`signed_fits`] so that
/// overflow errors can be raised with the correct argument position.
fn encode_int(buf: &mut Vec<u8>, val: i64, size: usize, endian: Endian) {
    let wide = val as i128;
    if size <= 8 {
        match endian {
            Endian::Little => buf.put_int_le(val, size),
            Endian::Big => buf.put_int(val, size),
        }
    } else {
        // Widen to i128, serialize all 16 bytes, then take the
        // relevant `size` slice.
        let all = match endian {
            Endian::Little => wide.to_le_bytes(),
            Endian::Big => wide.to_be_bytes(),
        };
        match endian {
            Endian::Little => buf.put_slice(&all[..size]),
            Endian::Big => buf.put_slice(&all[16 - size..]),
        }
    }
}

/// Encode an unsigned integer `val` into `size` bytes with the given endianness.
/// `val` is passed as i64 but treated as u64.
///
/// Callers are responsible for range-checking via [`unsigned_fits`].
fn encode_uint(buf: &mut Vec<u8>, val: i64, size: usize, endian: Endian) {
    let uval = val as u64;
    if size <= 8 {
        match endian {
            Endian::Little => buf.put_uint_le(uval, size),
            Endian::Big => buf.put_uint(uval, size),
        }
    } else {
        let wide = uval as u128;
        let all = match endian {
            Endian::Little => wide.to_le_bytes(),
            Endian::Big => wide.to_be_bytes(),
        };
        match endian {
            Endian::Little => buf.put_slice(&all[..size]),
            Endian::Big => buf.put_slice(&all[16 - size..]),
        }
    }
}

/// Decode a signed integer from `size` bytes with the given endianness.
fn decode_int(data: &mut &[u8], size: usize, endian: Endian) -> i64 {
    if size <= 8 {
        match endian {
            Endian::Little => data.get_int_le(size),
            Endian::Big => data.get_int(size),
        }
    } else {
        // Read `size` bytes into a 16-byte buffer, sign-extending.
        let mut all = [0u8; 16];
        let chunk = &data[..size];
        // Determine sign bit for extension.
        let sign_byte = match endian {
            Endian::Little => chunk[size - 1],
            Endian::Big => chunk[0],
        };
        let fill = if sign_byte & 0x80 != 0 { 0xFF } else { 0x00 };
        all.fill(fill);
        match endian {
            Endian::Little => all[..size].copy_from_slice(chunk),
            Endian::Big => all[16 - size..].copy_from_slice(chunk),
        }
        data.advance(size);
        let wide = match endian {
            Endian::Little => i128::from_le_bytes(all),
            Endian::Big => i128::from_be_bytes(all),
        };
        wide as i64
    }
}

/// Decode an unsigned integer from `size` bytes with the given endianness.
/// Returns as i64 (Lua integers are signed, but the bit pattern is unsigned).
fn decode_uint(data: &mut &[u8], size: usize, endian: Endian) -> Result<i64, VmError> {
    if size <= 8 {
        let uval = match endian {
            Endian::Little => data.get_uint_le(size),
            Endian::Big => data.get_uint(size),
        };
        Ok(uval as i64)
    } else {
        // Read into a zeroed 16-byte buffer.
        let mut all = [0u8; 16];
        let chunk = &data[..size];
        match endian {
            Endian::Little => all[..size].copy_from_slice(chunk),
            Endian::Big => all[16 - size..].copy_from_slice(chunk),
        }
        data.advance(size);
        let wide = match endian {
            Endian::Little => u128::from_le_bytes(all),
            Endian::Big => u128::from_be_bytes(all),
        };
        if wide > u64::MAX as u128 {
            return Err(pack_error(format!(
                "{}-byte integer does not fit into Lua Integer",
                size
            )));
        }
        Ok(wide as i64)
    }
}

// =========================================================================
// Public API
// =========================================================================

/// `string.pack(fmt, v1, v2, ...)` — pack values into a binary string.
pub fn string_pack(fmt: &[u8], args: &[Value]) -> Result<Vec<u8>, VmError> {
    let mut parser = FmtParser::new(fmt, "pack");
    let mut result: Vec<u8> = Vec::new();
    let mut arg_idx: usize = 0;

    while let Some(opt) = parser.next_opt()? {
        // Add alignment padding.
        let pad = parser.align_padding(&opt, result.len())?;
        result.extend(std::iter::repeat_n(0u8, pad));

        match opt {
            FmtOpt::Int { size } => {
                let val = get_int_arg(args, &mut arg_idx, "pack")?;
                if !signed_fits(val, size) {
                    return Err(arg_error(
                        "pack",
                        ArgPos::Value(arg_idx - 1),
                        "integer overflow",
                    ));
                }
                encode_int(&mut result, val, size, parser.endian);
            }
            FmtOpt::Uint { size } => {
                let val = get_int_arg(args, &mut arg_idx, "pack")?;
                if !unsigned_fits(val, size) {
                    return Err(arg_error(
                        "pack",
                        ArgPos::Value(arg_idx - 1),
                        "unsigned overflow",
                    ));
                }
                encode_uint(&mut result, val, size, parser.endian);
            }
            FmtOpt::Float => {
                let val = get_float_arg(args, &mut arg_idx, "pack")?;
                match parser.endian {
                    Endian::Little => result.put_f32_le(val as f32),
                    Endian::Big => result.put_f32(val as f32),
                }
            }
            FmtOpt::Double | FmtOpt::LuaNumber => {
                let val = get_float_arg(args, &mut arg_idx, "pack")?;
                match parser.endian {
                    Endian::Little => result.put_f64_le(val),
                    Endian::Big => result.put_f64(val),
                }
            }
            FmtOpt::FixedStr { n } => {
                let s = get_str_arg(args, &mut arg_idx, "pack")?;
                if s.len() > n {
                    return Err(arg_error(
                        "pack",
                        ArgPos::Value(arg_idx - 1),
                        "string longer than given size",
                    ));
                }
                result.put_slice(&s);
                // Pad with zeros if shorter.
                result.extend(std::iter::repeat_n(0u8, n - s.len()));
            }
            FmtOpt::ZStr => {
                let s = get_str_arg(args, &mut arg_idx, "pack")?;
                if s.iter().any(|&b| b == 0) {
                    return Err(arg_error(
                        "pack",
                        ArgPos::Value(arg_idx - 1),
                        "string contains zeros",
                    ));
                }
                result.put_slice(&s);
                result.put_u8(0);
            }
            FmtOpt::LenStr { len_size } => {
                let s = get_str_arg(args, &mut arg_idx, "pack")?;
                // Check that the string length fits in the given prefix
                // size. Lua's error message here is distinct from plain
                // integer overflow.
                if len_size < 8 {
                    let max = (1u64 << (len_size * 8)) - 1;
                    if (s.len() as u64) > max {
                        return Err(arg_error(
                            "pack",
                            ArgPos::Value(arg_idx - 1),
                            "string length does not fit in given size",
                        ));
                    }
                }
                let len = s.len() as i64;
                encode_uint(&mut result, len, len_size, parser.endian);
                result.put_slice(&s);
            }
            FmtOpt::Padding => {
                result.put_u8(0);
            }
            FmtOpt::AlignOnly { .. } => {
                // Alignment padding was already added above.
            }
        }
    }

    Ok(result)
}

/// `string.unpack(fmt, s [, pos])` — unpack values from a binary string.
///
/// Returns the unpacked values followed by the next read position (1-based).
///
/// `init_pos` follows Lua's string-index conventions: negative values count
/// from the end, values < 1 clamp to 1, and values > `s.len() + 1` error with
/// `"initial position out of string"`.
pub fn string_unpack(fmt: &[u8], s: &[u8], init_pos: i64) -> Result<Vec<Value>, VmError> {
    let len = s.len() as i64;
    let normalized = if init_pos < 0 {
        len + init_pos + 1
    } else {
        init_pos
    };
    if normalized > len + 1 {
        return Err(arg_error(
            "unpack",
            ArgPos::InitPos,
            "initial position out of string",
        ));
    }
    // Clamp to 1 (Lua accepts 0 and deeply negative values here).
    let start = std::cmp::max(normalized, 1) as usize;
    let mut offset = start - 1;
    let mut parser = FmtParser::new(fmt, "unpack");
    let mut results: Vec<Value> = Vec::new();

    while let Some(opt) = parser.next_opt()? {
        // Add alignment padding.
        let pad = parser.align_padding(&opt, offset)?;
        offset += pad;

        match opt {
            FmtOpt::Int { size } => {
                check_remaining(s, offset, size)?;
                let mut cursor = &s[offset..];
                let val = decode_int(&mut cursor, size, parser.endian);
                results.push(Value::Integer(val));
                offset += size;
            }
            FmtOpt::Uint { size } => {
                check_remaining(s, offset, size)?;
                let mut cursor = &s[offset..];
                let val = decode_uint(&mut cursor, size, parser.endian)?;
                results.push(Value::Integer(val));
                offset += size;
            }
            FmtOpt::Float => {
                check_remaining(s, offset, NATIVE_FLOAT)?;
                let mut cursor = &s[offset..];
                let val = match parser.endian {
                    Endian::Little => cursor.get_f32_le(),
                    Endian::Big => cursor.get_f32(),
                };
                results.push(Value::Float(val as f64));
                offset += NATIVE_FLOAT;
            }
            FmtOpt::Double | FmtOpt::LuaNumber => {
                check_remaining(s, offset, NATIVE_DOUBLE)?;
                let mut cursor = &s[offset..];
                let val = match parser.endian {
                    Endian::Little => cursor.get_f64_le(),
                    Endian::Big => cursor.get_f64(),
                };
                results.push(Value::Float(val));
                offset += NATIVE_DOUBLE;
            }
            FmtOpt::FixedStr { n } => {
                check_remaining(s, offset, n)?;
                results.push(Value::String(Bytes::from(&s[offset..offset + n])));
                offset += n;
            }
            FmtOpt::ZStr => {
                // Find the next zero byte.
                let start = offset;
                while offset < s.len() && s[offset] != 0 {
                    offset += 1;
                }
                if offset >= s.len() {
                    return Err(arg_error(
                        "unpack",
                        ArgPos::Data,
                        "unfinished string for format 'z'",
                    ));
                }
                results.push(Value::String(Bytes::from(&s[start..offset])));
                offset += 1; // skip the zero terminator
            }
            FmtOpt::LenStr { len_size } => {
                check_remaining(s, offset, len_size)?;
                let mut cursor = &s[offset..];
                let len = decode_uint(&mut cursor, len_size, parser.endian)? as usize;
                offset += len_size;
                check_remaining(s, offset, len)?;
                results.push(Value::String(Bytes::from(&s[offset..offset + len])));
                offset += len;
            }
            FmtOpt::Padding => {
                check_remaining(s, offset, 1)?;
                offset += 1;
            }
            FmtOpt::AlignOnly { .. } => {
                // Alignment padding was already added above.
            }
        }
    }

    // Append the next read position (1-based).
    results.push(Value::Integer((offset + 1) as i64));
    Ok(results)
}

/// `string.packsize(fmt)` — compute the total size of a packed string.
///
/// Variable-length options (`s` and `z`) are not allowed.
pub fn string_packsize(fmt: &[u8]) -> Result<i64, VmError> {
    let mut parser = FmtParser::new(fmt, "packsize");
    let mut total: usize = 0;

    while let Some(opt) = parser.next_opt()? {
        if opt.is_variable_length() {
            return Err(arg_error(
                "packsize",
                ArgPos::Format,
                "variable-length format",
            ));
        }
        // Add alignment padding.
        let pad = parser.align_padding(&opt, total)?;
        total += pad;
        match &opt {
            FmtOpt::Int { size } | FmtOpt::Uint { size } => total += size,
            FmtOpt::Float => total += NATIVE_FLOAT,
            FmtOpt::Double | FmtOpt::LuaNumber => total += NATIVE_DOUBLE,
            FmtOpt::FixedStr { n } => total += n,
            FmtOpt::Padding => total += 1,
            FmtOpt::AlignOnly { .. } => { /* alignment only */ }
            // Variable-length already rejected above.
            FmtOpt::ZStr | FmtOpt::LenStr { .. } => unreachable!(),
        }
    }

    Ok(total as i64)
}

// =========================================================================
// Argument helpers
// =========================================================================

fn get_int_arg(args: &[Value], idx: &mut usize, func: &str) -> Result<i64, VmError> {
    let i = *idx;
    *idx += 1;
    // Lua auto-coerces numeric strings and whole-valued floats for pack
    // integer slots. `coerce_to_integer` implements the same rules used
    // by `string.format`.  A missing argument is treated as `nil`
    // (matching Lua's stack-reads-past-top semantics), so the "got nil"
    // error text flows naturally from the coerce helper.
    let arg = args.get(i).unwrap_or(&Value::Nil);
    crate::string_lib::coerce_to_integer(arg, i + 2, func)
}

fn get_float_arg(args: &[Value], idx: &mut usize, func: &str) -> Result<f64, VmError> {
    let i = *idx;
    *idx += 1;
    let arg = args.get(i).unwrap_or(&Value::Nil);
    crate::string_lib::coerce_to_float(arg, i + 2, func)
}

/// Pack-specific string coercion. Accepts `String`/`Integer`/`Float`
/// (numbers stringified as Lua's `tostring` would), rejects `nil`,
/// `boolean`, `table`, etc.  Missing args are treated as `nil` so the
/// error text matches `string.pack("c3")` in reference Lua.
fn get_str_arg(args: &[Value], idx: &mut usize, func: &str) -> Result<Bytes, VmError> {
    let i = *idx;
    *idx += 1;
    match args.get(i).unwrap_or(&Value::Nil) {
        Value::String(s) => Ok(s.clone()),
        Value::Integer(n) => Ok(Bytes::from(n.to_string().into_bytes())),
        Value::Float(f) => Ok(Bytes::from(format!("{}", f).into_bytes())),
        other => Err(VmError::BadArgument {
            position: i + 2,
            function: func.to_owned(),
            expected: "string".to_owned(),
            got: other.type_name().to_owned(),
        }),
    }
}

fn check_remaining(s: &[u8], offset: usize, need: usize) -> Result<(), VmError> {
    if offset + need > s.len() {
        Err(arg_error("unpack", ArgPos::Data, "data string too short"))
    } else {
        Ok(())
    }
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(fmt: &str, args: Vec<Value>) -> Vec<u8> {
        string_pack(fmt.as_bytes(), &args).expect("pack should succeed")
    }

    fn unpack(fmt: &str, data: &[u8]) -> Vec<Value> {
        string_unpack(fmt.as_bytes(), data, 1).expect("unpack should succeed")
    }

    fn unpack_at(fmt: &str, data: &[u8], pos: i64) -> Result<Vec<Value>, VmError> {
        string_unpack(fmt.as_bytes(), data, pos)
    }

    fn packsize(fmt: &str) -> i64 {
        string_packsize(fmt.as_bytes()).expect("packsize should succeed")
    }

    // -- packsize --

    #[test]
    fn packsize_byte() {
        k9::assert_equal!(packsize("bB"), 2);
    }

    #[test]
    fn packsize_short() {
        k9::assert_equal!(packsize("hH"), 4);
    }

    #[test]
    fn packsize_int_default() {
        k9::assert_equal!(packsize("iI"), 8);
    }

    #[test]
    fn packsize_int_explicit() {
        k9::assert_equal!(packsize("i2I2"), 4);
    }

    #[test]
    fn packsize_long() {
        k9::assert_equal!(packsize("lL"), 16);
    }

    #[test]
    fn packsize_lua_integer() {
        k9::assert_equal!(packsize("jJ"), 16);
    }

    #[test]
    fn packsize_size_t() {
        k9::assert_equal!(packsize("T"), 8);
    }

    #[test]
    fn packsize_float_double() {
        k9::assert_equal!(packsize("fdn"), 20);
    }

    #[test]
    fn packsize_fixed_string() {
        k9::assert_equal!(packsize("c5c3"), 8);
    }

    #[test]
    fn packsize_padding() {
        k9::assert_equal!(packsize("bxb"), 3);
    }

    #[test]
    fn packsize_variable_length_error() {
        let err = string_packsize(b"z").unwrap_err().to_string();
        k9::assert_equal!(
            err,
            "bad argument #1 to 'packsize' (variable-length format)"
        );
    }

    #[test]
    fn packsize_variable_length_s_error() {
        let err = string_packsize(b"s").unwrap_err().to_string();
        k9::assert_equal!(
            err,
            "bad argument #1 to 'packsize' (variable-length format)"
        );
    }

    // -- pack / unpack round-trip --

    #[test]
    fn pack_unpack_signed_byte() {
        let data = pack("b", vec![Value::Integer(-1)]);
        k9::assert_equal!(data, vec![0xFF]);
        let vals = unpack("b", &data);
        k9::assert_equal!(vals, vec![Value::Integer(-1), Value::Integer(2)]);
    }

    #[test]
    fn pack_unpack_unsigned_byte() {
        let data = pack("B", vec![Value::Integer(255)]);
        k9::assert_equal!(data, vec![0xFF]);
        let vals = unpack("B", &data);
        k9::assert_equal!(vals, vec![Value::Integer(255), Value::Integer(2)]);
    }

    #[test]
    fn pack_unpack_little_endian_i2() {
        let data = pack("<i2", vec![Value::Integer(0x0102)]);
        k9::assert_equal!(data, vec![0x02, 0x01]);
        let vals = unpack("<i2", &data);
        k9::assert_equal!(vals, vec![Value::Integer(0x0102), Value::Integer(3)]);
    }

    #[test]
    fn pack_unpack_big_endian_i2() {
        let data = pack(">i2", vec![Value::Integer(0x0102)]);
        k9::assert_equal!(data, vec![0x01, 0x02]);
        let vals = unpack(">i2", &data);
        k9::assert_equal!(vals, vec![Value::Integer(0x0102), Value::Integer(3)]);
    }

    #[test]
    fn pack_unpack_float() {
        let data = pack("<f", vec![Value::Float(1.0)]);
        k9::assert_equal!(data, 1.0f32.to_le_bytes().to_vec());
        let vals = unpack("<f", &data);
        // Float round-trip: compare the float value.
        match &vals[0] {
            Value::Float(f) => assert!((*f - 1.0).abs() < 1e-6),
            other => panic!("expected float, got {:?}", other),
        }
    }

    #[test]
    fn pack_unpack_double() {
        let data = pack("<d", vec![Value::Float(std::f64::consts::PI)]);
        k9::assert_equal!(data, std::f64::consts::PI.to_le_bytes().to_vec());
        let vals = unpack("<d", &data);
        k9::assert_equal!(vals[0], Value::Float(std::f64::consts::PI));
    }

    #[test]
    fn pack_unpack_fixed_string() {
        let data = pack("c5", vec![Value::string("hi")]);
        k9::assert_equal!(data, vec![b'h', b'i', 0, 0, 0]);
        let vals = unpack("c5", &data);
        k9::assert_equal!(vals, vec![Value::string("hi\0\0\0"), Value::Integer(6),]);
    }

    #[test]
    fn pack_unpack_zstring() {
        let data = pack("z", vec![Value::string("hello")]);
        k9::assert_equal!(data, b"hello\0".to_vec());
        let vals = unpack("z", &data);
        k9::assert_equal!(vals, vec![Value::string("hello"), Value::Integer(7),]);
    }

    #[test]
    fn pack_unpack_len_string() {
        let data = pack("<s4", vec![Value::string("ab")]);
        // 4-byte little-endian length (2) + "ab"
        k9::assert_equal!(data, vec![2, 0, 0, 0, b'a', b'b']);
        let vals = unpack("<s4", &data);
        k9::assert_equal!(vals, vec![Value::string("ab"), Value::Integer(7),]);
    }

    #[test]
    fn pack_unpack_multiple_values() {
        let data = pack("<i4d", vec![Value::Integer(42), Value::Float(2.5)]);
        let mut expected = 42i32.to_le_bytes().to_vec();
        expected.extend_from_slice(&2.5f64.to_le_bytes());
        k9::assert_equal!(data, expected);
        let vals = unpack("<i4d", &data);
        k9::assert_equal!(
            vals,
            vec![Value::Integer(42), Value::Float(2.5), Value::Integer(13),]
        );
    }

    #[test]
    fn pack_overflow_signed() {
        let err = string_pack(b"b", &[Value::Integer(200)])
            .unwrap_err()
            .to_string();
        k9::assert_equal!(err, "bad argument #2 to 'pack' (integer overflow)");
    }

    #[test]
    fn pack_overflow_unsigned() {
        let err = string_pack(b"B", &[Value::Integer(-1)])
            .unwrap_err()
            .to_string();
        // -1 as u64 = 18446744073709551615, which doesn't fit in 1 byte.
        k9::assert_equal!(err, "bad argument #2 to 'pack' (unsigned overflow)");
    }

    #[test]
    fn pack_zstring_with_zero() {
        let err = string_pack(b"z", &[Value::string("he\0lo")])
            .unwrap_err()
            .to_string();
        k9::assert_equal!(err, "bad argument #2 to 'pack' (string contains zeros)");
    }

    #[test]
    fn unpack_data_too_short() {
        let err = string_unpack(b"i4", &[1, 2], 1).unwrap_err().to_string();
        k9::assert_equal!(err, "bad argument #2 to 'unpack' (data string too short)");
    }

    #[test]
    fn unpack_with_offset() {
        // Pack two i2 values, unpack starting at the second.
        let data = pack("<i2i2", vec![Value::Integer(1), Value::Integer(2)]);
        let vals = string_unpack(b"<i2", &data, 3).expect("unpack");
        k9::assert_equal!(vals, vec![Value::Integer(2), Value::Integer(5)]);
    }

    #[test]
    fn packsize_with_alignment() {
        // !4 sets max alignment to 4. Packing b then i4: 1 byte + 3 pad + 4 bytes = 8.
        k9::assert_equal!(packsize("!4 bi4"), 8);
    }

    #[test]
    fn pack_unpack_with_alignment() {
        let data = pack(
            "!4 bi4",
            vec![Value::Integer(1), Value::Integer(0x12345678)],
        );
        k9::assert_equal!(data, vec![1, 0, 0, 0, 0x78, 0x56, 0x34, 0x12]);
        let vals = unpack("!4 bi4", &data);
        k9::assert_equal!(
            vals,
            vec![
                Value::Integer(1),
                Value::Integer(0x12345678),
                Value::Integer(9),
            ]
        );
    }

    #[test]
    fn invalid_format_option() {
        let err = string_pack(b"Q", &[]).unwrap_err().to_string();
        k9::assert_equal!(err, "invalid format option 'Q'");
    }

    #[test]
    fn packsize_int_size_out_of_limits() {
        let err = string_packsize(b"i0").unwrap_err().to_string();
        k9::assert_equal!(err, "integral size (0) out of limits [1,16]");
    }

    #[test]
    fn packsize_int_size_too_large() {
        let err = string_packsize(b"i17").unwrap_err().to_string();
        k9::assert_equal!(err, "integral size (17) out of limits [1,16]");
    }

    #[test]
    fn pack_negative_i2() {
        let data = pack("<i2", vec![Value::Integer(-1)]);
        k9::assert_equal!(data, vec![0xFF, 0xFF]);
        let vals = unpack("<i2", &data);
        k9::assert_equal!(vals, vec![Value::Integer(-1), Value::Integer(3)]);
    }

    #[test]
    fn pack_unpack_lua_integer_j() {
        let data = pack("<j", vec![Value::Integer(i64::MAX)]);
        k9::assert_equal!(data, i64::MAX.to_le_bytes().to_vec());
        let vals = unpack("<j", &data);
        k9::assert_equal!(vals, vec![Value::Integer(i64::MAX), Value::Integer(9)]);
    }

    #[test]
    fn pack_spaces_ignored() {
        let data = pack("< b b", vec![Value::Integer(1), Value::Integer(2)]);
        k9::assert_equal!(data, vec![1, 2]);
    }

    #[test]
    fn pack_unpack_x_align() {
        // Xd aligns to double (8 bytes) without consuming a value.
        let data = pack("!8 b Xd i4", vec![Value::Integer(1), Value::Integer(42)]);
        // byte(1) + 7 pad to align to 8 + i4(42) = 12 bytes
        k9::assert_equal!(data, vec![1, 0, 0, 0, 0, 0, 0, 0, 42, 0, 0, 0]);
        let vals = unpack("!8 b Xd i4", &data);
        k9::assert_equal!(
            vals,
            vec![Value::Integer(1), Value::Integer(42), Value::Integer(13),]
        );
    }

    // ------------------------------------------------------------------
    // Reference Lua 5.4.4 compatibility tests.
    //
    // The expected byte sequences in these tests were produced by running
    // `string.pack` under Lua 5.4.4 (`/usr/bin/lua` on the dev machine).
    // Each test verifies that:
    //   (a) unpacking the reference bytes yields the expected Lua values, and
    //   (b) packing the same values produces byte-for-byte identical output.
    // This guards against subtle endianness, sign-extension, alignment,
    // and string-framing bugs that would otherwise only surface when
    // exchanging packed data with real Lua programs.
    // ------------------------------------------------------------------

    /// Decode a lowercase hex string into bytes. Test helper only.
    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
            .collect()
    }

    /// Assert that packing `args` with `fmt` matches `expected_hex`, and
    /// that unpacking `expected_hex` recovers `args` (plus a trailing
    /// position value).
    fn check_reference(fmt: &str, args: Vec<Value>, expected_hex: &str) {
        let expected = hex(expected_hex);
        let packed = pack(fmt, args.clone());
        k9::assert_equal!(packed, expected, "pack({fmt}) mismatch");

        let mut expected_unpack = args;
        expected_unpack.push(Value::Integer((expected.len() + 1) as i64));
        let unpacked = unpack(fmt, &expected);
        k9::assert_equal!(unpacked, expected_unpack, "unpack({fmt}) mismatch");
    }

    #[test]
    fn reference_i8_positive() {
        check_reference("b", vec![Value::Integer(42)], "2a");
    }

    #[test]
    fn reference_i8_negative() {
        check_reference("b", vec![Value::Integer(-1)], "ff");
    }

    #[test]
    fn reference_i8_min() {
        check_reference("b", vec![Value::Integer(-128)], "80");
    }

    #[test]
    fn reference_i8_max() {
        check_reference("b", vec![Value::Integer(127)], "7f");
    }

    #[test]
    fn reference_u8_max() {
        check_reference("B", vec![Value::Integer(255)], "ff");
    }

    #[test]
    fn reference_i16_le() {
        check_reference("<h", vec![Value::Integer(1000)], "e803");
    }

    #[test]
    fn reference_i16_be() {
        check_reference(">h", vec![Value::Integer(1000)], "03e8");
    }

    #[test]
    fn reference_i16_le_negative() {
        check_reference("<h", vec![Value::Integer(-1000)], "18fc");
    }

    #[test]
    fn reference_i16_le_neg_one() {
        check_reference("<i2", vec![Value::Integer(-1)], "ffff");
    }

    #[test]
    fn reference_u16_le_max() {
        check_reference("<H", vec![Value::Integer(65535)], "ffff");
    }

    #[test]
    fn reference_i32_le_min() {
        check_reference("<i4", vec![Value::Integer(i32::MIN as i64)], "00000080");
    }

    #[test]
    fn reference_i32_be_min() {
        check_reference(">i4", vec![Value::Integer(i32::MIN as i64)], "80000000");
    }

    #[test]
    fn reference_u32_le_max() {
        check_reference("<I4", vec![Value::Integer(u32::MAX as i64)], "ffffffff");
    }

    #[test]
    fn reference_i64_le_min() {
        check_reference("<l", vec![Value::Integer(i64::MIN)], "0000000000000080");
    }

    #[test]
    fn reference_i64_be_max() {
        check_reference(">l", vec![Value::Integer(i64::MAX)], "7fffffffffffffff");
    }

    #[test]
    fn reference_u64_le_all_ones() {
        // Lua 5.4 wraps unsigned 0xFFFFFFFFFFFFFFFF to signed -1 because
        // lua_Integer is i64. Our representation matches.
        check_reference("<L", vec![Value::Integer(-1)], "ffffffffffffffff");
    }

    #[test]
    fn reference_i3_le_max() {
        check_reference("<i3", vec![Value::Integer(8388607)], "ffff7f");
    }

    #[test]
    fn reference_i3_le_min() {
        check_reference("<i3", vec![Value::Integer(-8388608)], "000080");
    }

    #[test]
    fn reference_u3_le_max() {
        check_reference("<I3", vec![Value::Integer(16777215)], "ffffff");
    }

    #[test]
    fn reference_u7_le_max() {
        check_reference(
            "<I7",
            vec![Value::Integer(72057594037927935)],
            "ffffffffffffff",
        );
    }

    #[test]
    fn reference_i16byte_zero() {
        check_reference(
            "<i16",
            vec![Value::Integer(0)],
            "00000000000000000000000000000000",
        );
    }

    #[test]
    fn reference_i16byte_neg_one() {
        // Sign-extends i64(-1) to 16 bytes as all 0xFF.
        check_reference(
            "<i16",
            vec![Value::Integer(-1)],
            "ffffffffffffffffffffffffffffffff",
        );
    }

    #[test]
    fn reference_f32_le() {
        check_reference("<f", vec![Value::Float(1.5)], "0000c03f");
    }

    #[test]
    fn reference_f32_be() {
        check_reference(">f", vec![Value::Float(1.5)], "3fc00000");
    }

    #[test]
    fn reference_f64_le() {
        check_reference("<d", vec![Value::Float(1.5)], "000000000000f83f");
    }

    #[test]
    fn reference_f64_be() {
        check_reference(">d", vec![Value::Float(1.5)], "3ff8000000000000");
    }

    #[test]
    fn reference_f64_le_pi() {
        check_reference(
            "<d",
            vec![Value::Float(std::f64::consts::PI)],
            "182d4454fb210940",
        );
    }

    #[test]
    fn reference_fixed_string_exact() {
        check_reference("c5", vec![Value::string("hello")], "68656c6c6f");
    }

    #[test]
    fn reference_fixed_string_padded() {
        // unpack returns the full 10-byte contents (including trailing NULs).
        // pack requires the input to be <= 10 bytes and pads with NULs.
        let packed = pack("c10", vec![Value::string("hi")]);
        k9::assert_equal!(packed, hex("68690000000000000000"));
        let vals = unpack("c10", &packed);
        k9::assert_equal!(
            vals,
            vec![Value::string("hi\0\0\0\0\0\0\0\0"), Value::Integer(11),]
        );
    }

    #[test]
    fn reference_zstring() {
        check_reference("z", vec![Value::string("abc")], "61626300");
    }

    #[test]
    fn reference_s1_string() {
        check_reference("<s1", vec![Value::string("hello")], "0568656c6c6f");
    }

    #[test]
    fn reference_s2_string() {
        check_reference("<s2", vec![Value::string("hi")], "02006869");
    }

    #[test]
    fn reference_s4_string() {
        check_reference("<s4", vec![Value::string("world!")], "06000000776f726c6421");
    }

    #[test]
    fn reference_s8_string() {
        check_reference("<s8", vec![Value::string("x")], "010000000000000078");
    }

    #[test]
    fn reference_three_bytes() {
        check_reference(
            "bbb",
            vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)],
            "010203",
        );
    }

    #[test]
    fn reference_mixed_le() {
        check_reference(
            "<i4i2b",
            vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)],
            "01000000020003",
        );
    }

    #[test]
    fn reference_mixed_be() {
        check_reference(
            ">i4i2b",
            vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)],
            "00000001000203",
        );
    }

    #[test]
    fn reference_padding() {
        check_reference("bxb", vec![Value::Integer(1), Value::Integer(2)], "010002");
    }

    #[test]
    fn reference_align_bang_4() {
        check_reference(
            "<!4 b i4",
            vec![Value::Integer(1), Value::Integer(42)],
            "010000002a000000",
        );
    }

    #[test]
    fn reference_align_bang_8() {
        check_reference(
            "<!8 b i8",
            vec![Value::Integer(1), Value::Integer(42)],
            "01000000000000002a00000000000000",
        );
    }

    #[test]
    fn reference_align_bang_2_i2() {
        check_reference(
            "<!2 b i2",
            vec![Value::Integer(1), Value::Integer(256)],
            "01000001",
        );
    }

    #[test]
    fn reference_xop_default_alignment() {
        // Default max alignment is 1, so `Xi4` adds no padding.
        check_reference(
            "<b Xi4 i4",
            vec![Value::Integer(1), Value::Integer(42)],
            "012a000000",
        );
    }

    // ------------------------------------------------------------------
    // Additional reference Lua 5.4.4 tests for format options and
    // combinations that the first batch didn't cover. Hex values here
    // were also captured from `/usr/bin/lua` running `string.pack`.
    // ------------------------------------------------------------------

    #[test]
    fn reference_size_t_le() {
        check_reference("<T", vec![Value::Integer(12345)], "3930000000000000");
    }

    #[test]
    fn reference_size_t_be() {
        check_reference(">T", vec![Value::Integer(12345)], "0000000000003039");
    }

    #[test]
    fn reference_lua_integer_j_le() {
        check_reference("<j", vec![Value::Integer(1234567890)], "d202964900000000");
    }

    #[test]
    fn reference_lua_integer_j_be_negative() {
        check_reference(">j", vec![Value::Integer(-1234567890)], "ffffffffb669fd2e");
    }

    #[test]
    fn reference_lua_integer_big_j_le() {
        check_reference("<J", vec![Value::Integer(0xDEADBEEF)], "efbeadde00000000");
    }

    #[test]
    fn reference_lua_number_le() {
        check_reference(
            "<n",
            vec![Value::Float(std::f64::consts::E)],
            "6957148b0abf0540",
        );
    }

    #[test]
    fn reference_lua_number_be() {
        check_reference(
            ">n",
            vec![Value::Float(std::f64::consts::E)],
            "4005bf0a8b145769",
        );
    }

    #[test]
    fn reference_int_no_size() {
        // `i` without an explicit size uses native int size (4 on this
        // platform). Matches Lua's default.
        check_reference("<i", vec![Value::Integer(42)], "2a000000");
    }

    #[test]
    fn reference_uint_no_size() {
        check_reference("<I", vec![Value::Integer(42)], "2a000000");
    }

    #[test]
    fn reference_s_no_size() {
        // `s` without an explicit size uses native size_t (8 bytes).
        check_reference("<s", vec![Value::string("hi")], "02000000000000006869");
    }

    #[test]
    fn reference_default_endian_is_native() {
        // No endian prefix. On this little-endian host, `i4` of
        // 0x12345678 packs as the bytes 78 56 34 12 (LE).
        check_reference("i4", vec![Value::Integer(0x12345678)], "78563412");
    }

    #[test]
    fn reference_equals_resets_to_native() {
        // `<i4=i4` packs first value LE, then switches back to native.
        // On LE host, both end up LE.
        check_reference(
            "<i4=i4",
            vec![Value::Integer(1), Value::Integer(2)],
            "0100000002000000",
        );
    }

    #[test]
    fn reference_bare_bang_native_alignment() {
        // `!` with no number uses the platform's natural alignment
        // (size_of(long long) = 8 on x86_64). The following i8 gets
        // aligned to offset 8.
        check_reference(
            "! b i8",
            vec![Value::Integer(1), Value::Integer(2)],
            "01000000000000000200000000000000",
        );
    }

    #[test]
    fn reference_mid_format_endian_swap() {
        // `<i2>i2`: first value LE, second value BE.
        check_reference(
            "<i2>i2",
            vec![Value::Integer(1), Value::Integer(1)],
            "01000001",
        );
    }

    #[test]
    fn reference_mid_format_alignment_change() {
        // Default alignment is 1, so `b` has no padding.
        // `!4` sets max alignment to 4, so subsequent `i4` aligns to 4.
        check_reference(
            "<b !4 i4",
            vec![Value::Integer(1), Value::Integer(42)],
            "010000002a000000",
        );
    }

    #[test]
    fn reference_f32_negative_zero() {
        // -0.0 has bit pattern 0x80000000 (LE: 00 00 00 80).
        let data = pack("<f", vec![Value::Float(-0.0)]);
        k9::assert_equal!(data, hex("00000080"));
        let vals = unpack("<f", &data);
        // Confirm sign bit is preserved on round-trip.
        match &vals[0] {
            Value::Float(f) => {
                k9::assert_equal!(f.is_sign_negative(), true);
                k9::assert_equal!(*f, 0.0);
            }
            other => panic!("expected float, got {other:?}"),
        }
    }

    #[test]
    fn reference_f64_positive_infinity() {
        check_reference("<d", vec![Value::Float(f64::INFINITY)], "000000000000f07f");
    }

    #[test]
    fn reference_f64_negative_infinity() {
        check_reference(
            "<d",
            vec![Value::Float(f64::NEG_INFINITY)],
            "000000000000f0ff",
        );
    }

    #[test]
    fn reference_f64_nan_roundtrip() {
        // Pack a specific NaN bit pattern and verify we unpack an equal
        // bit pattern (NaN != NaN, so compare as bits).
        let nan_bits: u64 = 0xfff8000000000000;
        let packed = pack("<d", vec![Value::Float(f64::from_bits(nan_bits))]);
        k9::assert_equal!(packed, hex("000000000000f8ff"));
        let vals = unpack("<d", &packed);
        match &vals[0] {
            Value::Float(f) => {
                k9::assert_equal!(f.to_bits(), nan_bits);
            }
            other => panic!("expected float, got {other:?}"),
        }
    }

    #[test]
    fn reference_c0_zero_fixed_string() {
        // Lua accepts c0 as a zero-byte fixed string.
        let data = pack("c0", vec![Value::string("")]);
        k9::assert_equal!(data, Vec::<u8>::new());
        k9::assert_equal!(packsize("c0"), 0);
        let vals = unpack("c0", &data);
        k9::assert_equal!(vals, vec![Value::string(""), Value::Integer(1)]);
    }

    // ------------------------------------------------------------------
    // Error path tests. Error messages match Lua 5.4's wording so that
    // scripts that inspect error strings behave the same.
    // ------------------------------------------------------------------

    #[test]
    fn err_missing_size_for_c() {
        let err = string_pack(b"c", &[]).unwrap_err().to_string();
        k9::assert_equal!(err, "missing size for format option 'c'");
    }

    #[test]
    fn err_x_at_end_of_format() {
        let err = string_pack(b"X", &[]).unwrap_err().to_string();
        k9::assert_equal!(
            err,
            "bad argument #1 to 'pack' (invalid next option for option 'X')"
        );
    }

    #[test]
    fn err_x_followed_by_x() {
        let err = string_pack(b"XX", &[]).unwrap_err().to_string();
        k9::assert_equal!(
            err,
            "bad argument #1 to 'pack' (invalid next option for option 'X')"
        );
    }

    #[test]
    fn err_x_followed_by_c() {
        // `c<n>` fixed strings have no meaningful alignment.
        let err = string_pack(b"Xc5", &[]).unwrap_err().to_string();
        k9::assert_equal!(
            err,
            "bad argument #1 to 'pack' (invalid next option for option 'X')"
        );
    }

    #[test]
    fn err_x_followed_by_z() {
        let err = string_pack(b"Xz", &[]).unwrap_err().to_string();
        k9::assert_equal!(
            err,
            "bad argument #1 to 'pack' (invalid next option for option 'X')"
        );
    }

    #[test]
    fn err_x_followed_by_space() {
        // Lua's `getoption` classifies space as `Knop`, which X rejects —
        // even though the outer loop would otherwise skip spaces silently.
        let err = string_pack(b"X i4", &[Value::Integer(1)])
            .unwrap_err()
            .to_string();
        k9::assert_equal!(
            err,
            "bad argument #1 to 'pack' (invalid next option for option 'X')"
        );
    }

    #[test]
    fn err_x_followed_by_endian() {
        for byte in [b'<', b'>', b'='] {
            let fmt = [b'X', byte, b'i', b'4'];
            let err = string_pack(&fmt, &[Value::Integer(1)])
                .unwrap_err()
                .to_string();
            k9::assert_equal!(
                err,
                "bad argument #1 to 'pack' (invalid next option for option 'X')",
                "fmt: X{}i4",
                byte as char
            );
        }
    }

    #[test]
    fn err_x_followed_by_bang() {
        let err = string_pack(b"X!4i4", &[Value::Integer(1)])
            .unwrap_err()
            .to_string();
        k9::assert_equal!(
            err,
            "bad argument #1 to 'pack' (invalid next option for option 'X')"
        );
    }

    #[test]
    fn reference_x_followed_by_x_padding_allowed() {
        // Xx is allowed by Lua: padding has a well-defined size of 1.
        // The resulting alignment is min(1, max_align) = 1, so Xx is
        // effectively a no-op in the default configuration.
        let data = pack("bXxb", vec![Value::Integer(1), Value::Integer(2)]);
        k9::assert_equal!(data, vec![1, 2]);
    }

    #[test]
    fn reference_x_followed_by_s_allowed() {
        // Xs1 aligns to the size of the length prefix (1 byte) — no
        // padding since that's also the current min alignment.
        let data = pack("bXs1", vec![Value::Integer(1), Value::string("hi")]);
        k9::assert_equal!(data, vec![1]);
    }

    #[test]
    fn err_string_longer_than_fixed_size() {
        let err = string_pack(b"c3", &[Value::string("hello")])
            .unwrap_err()
            .to_string();
        k9::assert_equal!(
            err,
            "bad argument #2 to 'pack' (string longer than given size)"
        );
    }

    #[test]
    fn err_signed_overflow_just_below_min() {
        let err = string_pack(b"b", &[Value::Integer(-129)])
            .unwrap_err()
            .to_string();
        k9::assert_equal!(err, "bad argument #2 to 'pack' (integer overflow)");
    }

    #[test]
    fn err_unfinished_z_string() {
        let err = string_unpack(b"z", b"abc", 1).unwrap_err().to_string();
        k9::assert_equal!(
            err,
            "bad argument #2 to 'unpack' (unfinished string for format 'z')"
        );
    }

    #[test]
    fn err_alignment_not_power_of_2_when_applied() {
        // `!3 b` succeeds because `b` has align=min(1,3)=1.
        // `!3 b i4` fails because i4's alignment is min(4,3)=3 which
        // isn't a power of 2.
        let ok = string_pack(b"!3 b", &[Value::Integer(1)]);
        k9::assert_equal!(ok.unwrap(), vec![1u8]);

        let err = string_pack(b"!3 b i4", &[Value::Integer(1), Value::Integer(2)])
            .unwrap_err()
            .to_string();
        k9::assert_equal!(
            err,
            "bad argument #1 to 'pack' (format asks for alignment not power of 2)"
        );
    }

    #[test]
    fn err_alignment_size_out_of_limits() {
        let err = string_packsize(b"!0").unwrap_err().to_string();
        k9::assert_equal!(err, "integral size (0) out of limits [1,16]");
    }

    #[test]
    fn err_unpack_initial_position_out_of_string() {
        // Position past len+1 errors even for a zero-length format.
        let err = unpack_at("", b"abc", 100).unwrap_err().to_string();
        k9::assert_equal!(
            err,
            "bad argument #3 to 'unpack' (initial position out of string)"
        );
    }

    #[test]
    fn err_unpack_data_string_too_short_at_boundary() {
        // Position == len+1 is valid for empty format, but errors for
        // anything that actually needs a byte.
        let err = unpack_at("b", b"abc", 4).unwrap_err().to_string();
        k9::assert_equal!(err, "bad argument #2 to 'unpack' (data string too short)");
    }

    #[test]
    fn reference_unpack_negative_position() {
        // Negative `init_pos` counts from the end of the string.
        let data = &[0x01, 0x02, 0x03, 0x04, 0x05][..];
        let vals = unpack_at("b", data, -1).expect("unpack");
        k9::assert_equal!(vals, vec![Value::Integer(5), Value::Integer(6)]);

        let vals = unpack_at("b", data, -3).expect("unpack");
        k9::assert_equal!(vals, vec![Value::Integer(3), Value::Integer(4)]);
    }

    #[test]
    fn reference_unpack_position_clamped_to_one() {
        // Deeply negative and zero both clamp to 1.
        let data = &[0x01, 0x02][..];
        for pos in [-100i64, -3, -2, 0, 1] {
            let vals = unpack_at("b", data, pos).expect("unpack");
            k9::assert_equal!(
                vals,
                vec![Value::Integer(1), Value::Integer(2)],
                "pos={pos}"
            );
        }
    }

    #[test]
    fn reference_unpack_position_at_end_of_string() {
        // pos == len+1 is valid for a zero-consumption format.
        let data = &[0x01, 0x02, 0x03][..];
        let vals = unpack_at("", data, 4).expect("unpack");
        k9::assert_equal!(vals, vec![Value::Integer(4)]);
    }

    // ------------------------------------------------------------------
    // Lua-style argument coercion. `string.pack` accepts numeric
    // strings for number slots, and stringifies numbers for string
    // slots; non-coercible types (nil, boolean, table) are rejected.
    // ------------------------------------------------------------------

    #[test]
    fn coerce_numeric_string_to_integer() {
        // '42' packs as byte 42 — same as Lua.
        let data = pack("b", vec![Value::string("42")]);
        k9::assert_equal!(data, vec![42u8]);
    }

    #[test]
    fn coerce_hex_string_to_integer() {
        // Lua's numeric-string parsing accepts `0x` hex literals.
        let data = pack("b", vec![Value::string("0x2a")]);
        k9::assert_equal!(data, vec![42u8]);
    }

    #[test]
    fn coerce_numeric_string_to_float() {
        let data = pack("<d", vec![Value::string("1.5")]);
        let vals = unpack("<d", &data);
        k9::assert_equal!(vals[0], Value::Float(1.5));
    }

    #[test]
    fn coerce_integer_to_string_fixed() {
        // `c3` with integer 42 packs as "42\0" (number stringified, padded).
        let data = pack("c3", vec![Value::Integer(42)]);
        k9::assert_equal!(data, vec![b'4', b'2', 0]);
    }

    #[test]
    fn coerce_float_to_string_zstr() {
        // Float for `z`: Lua stringifies, so "3.14" + NUL.
        let data = pack("z", vec![Value::Float(3.14)]);
        k9::assert_equal!(&data[..4], b"3.14");
        k9::assert_equal!(data[4], 0);
    }

    #[test]
    fn coerce_integer_to_len_string() {
        // Integer 42 stringified to "42", packed with 1-byte length prefix.
        let data = pack("<s1", vec![Value::Integer(42)]);
        k9::assert_equal!(data, vec![2, b'4', b'2']);
    }

    #[test]
    fn coerce_whole_float_to_integer_slot() {
        // Whole-valued floats pack fine — Lua's `lua_numbertointeger`
        // accepts them.
        let data = pack("b", vec![Value::Float(42.0)]);
        k9::assert_equal!(data, vec![42u8]);
    }

    #[test]
    fn reject_fractional_float_in_integer_slot() {
        // Fractional floats have no exact integer representation — Lua
        // errors rather than silently truncating.
        let err = string_pack(b"b", &[Value::Float(42.5)])
            .unwrap_err()
            .to_string();
        k9::assert_equal!(
            err,
            "bad argument #2 to 'pack' (number has no integer representation)"
        );
    }

    #[test]
    fn reject_infinity_in_integer_slot() {
        let err = string_pack(b"b", &[Value::Float(f64::INFINITY)])
            .unwrap_err()
            .to_string();
        k9::assert_equal!(
            err,
            "bad argument #2 to 'pack' (number has no integer representation)"
        );
    }

    #[test]
    fn reject_nan_in_integer_slot() {
        let err = string_pack(b"b", &[Value::Float(f64::NAN)])
            .unwrap_err()
            .to_string();
        k9::assert_equal!(
            err,
            "bad argument #2 to 'pack' (number has no integer representation)"
        );
    }

    #[test]
    fn reject_fractional_numeric_string_in_integer_slot() {
        // "3.5" parses as a float; since it has no integer representation,
        // Lua rejects it with the same message.
        let err = string_pack(b"b", &[Value::string("3.5")])
            .unwrap_err()
            .to_string();
        k9::assert_equal!(
            err,
            "bad argument #2 to 'pack' (number has no integer representation)"
        );
    }

    #[test]
    fn accept_whole_valued_numeric_string_with_decimals() {
        // "3.0" parses as 3.0 — a whole-valued float, so it passes the
        // integer-representation check.
        let data = pack("b", vec![Value::string("3.0")]);
        k9::assert_equal!(data, vec![3u8]);
    }

    #[test]
    fn accept_exponent_numeric_string_in_integer_slot() {
        // "1e2" = 100.0 — whole-valued float.
        let data = pack("b", vec![Value::string("1e2")]);
        k9::assert_equal!(data, vec![100u8]);
    }

    // ------------------------------------------------------------------
    // Non-coercible types must still error.
    // ------------------------------------------------------------------

    #[test]
    fn reject_nil_for_integer_slot() {
        let err = string_pack(b"b", &[Value::Nil]).unwrap_err().to_string();
        k9::assert_equal!(err, "bad argument #2 to 'pack' (number expected, got nil)");
    }

    #[test]
    fn reject_boolean_for_integer_slot() {
        let err = string_pack(b"b", &[Value::Boolean(true)])
            .unwrap_err()
            .to_string();
        k9::assert_equal!(
            err,
            "bad argument #2 to 'pack' (number expected, got boolean)"
        );
    }

    #[test]
    fn reject_non_numeric_string_for_integer_slot() {
        let err = string_pack(b"b", &[Value::string("abc")])
            .unwrap_err()
            .to_string();
        k9::assert_equal!(
            err,
            "bad argument #2 to 'pack' (number expected, got string)"
        );
    }

    #[test]
    fn reject_nil_for_string_slot() {
        let err = string_pack(b"c3", &[Value::Nil]).unwrap_err().to_string();
        k9::assert_equal!(err, "bad argument #2 to 'pack' (string expected, got nil)");
    }

    #[test]
    fn reject_boolean_for_string_slot() {
        let err = string_pack(b"z", &[Value::Boolean(false)])
            .unwrap_err()
            .to_string();
        k9::assert_equal!(
            err,
            "bad argument #2 to 'pack' (string expected, got boolean)"
        );
    }

    // ------------------------------------------------------------------
    // Length-prefix-string boundary cases. Lua's specific error message
    // for an overflowing length prefix differs from plain integer
    // overflow.
    // ------------------------------------------------------------------

    #[test]
    fn s1_at_max_length() {
        // 255 bytes fits in a 1-byte length prefix.
        let s = Bytes::from(vec![b'x'; 255]);
        let data = pack("<s1", vec![Value::String(s.clone())]);
        let mut expected = vec![255u8];
        expected.extend_from_slice(&vec![b'x'; 255]);
        k9::assert_equal!(data, expected);
        let vals = unpack("<s1", &data);
        k9::assert_equal!(vals, vec![Value::String(s), Value::Integer(257)]);
    }

    #[test]
    fn s1_length_does_not_fit() {
        let s = Bytes::from(vec![b'x'; 256]);
        let err = string_pack(b"<s1", &[Value::String(s)])
            .unwrap_err()
            .to_string();
        k9::assert_equal!(
            err,
            "bad argument #2 to 'pack' (string length does not fit in given size)"
        );
    }

    #[test]
    fn s2_at_max_length() {
        let s = Bytes::from(vec![b'x'; 65535]);
        let data = pack("<s2", vec![Value::String(s.clone())]);
        let mut expected = vec![0xFF, 0xFF];
        expected.extend_from_slice(&vec![b'x'; 65535]);
        k9::assert_equal!(data, expected);
        let vals = unpack("<s2", &data);
        k9::assert_equal!(vals, vec![Value::String(s), Value::Integer(65538)]);
    }

    #[test]
    fn s2_length_does_not_fit() {
        let s = Bytes::from(vec![b'x'; 65536]);
        let err = string_pack(b"<s2", &[Value::String(s)])
            .unwrap_err()
            .to_string();
        k9::assert_equal!(
            err,
            "bad argument #2 to 'pack' (string length does not fit in given size)"
        );
    }

    // ------------------------------------------------------------------
    // Whitespace handling, empty formats, and argument-count behavior.
    // ------------------------------------------------------------------

    #[test]
    fn empty_format_pack() {
        let data = pack("", vec![]);
        k9::assert_equal!(data, Vec::<u8>::new());
    }

    #[test]
    fn empty_format_packsize() {
        k9::assert_equal!(packsize(""), 0);
    }

    #[test]
    fn empty_format_unpack() {
        // Returns only the next-position value.
        let vals = unpack("", b"abc");
        k9::assert_equal!(vals, vec![Value::Integer(1)]);
    }

    #[test]
    fn whitespace_only_format_packsize() {
        k9::assert_equal!(packsize("   "), 0);
    }

    #[test]
    fn tab_in_format_rejected() {
        let err = string_packsize(b"\tb").unwrap_err().to_string();
        k9::assert_equal!(err, "invalid format option '\t'");
    }

    #[test]
    fn extra_pack_args_silently_ignored() {
        // Pack takes only as many args as the format needs.
        let data = pack(
            "b",
            vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)],
        );
        k9::assert_equal!(data, vec![1u8]);
    }

    #[test]
    fn config_only_format_packsize() {
        // Just endian/alignment directives and no data options.
        k9::assert_equal!(packsize("<>!8"), 0);
    }

    // ------------------------------------------------------------------
    // Float edge cases: overflow, subnormal, precision.
    // ------------------------------------------------------------------

    #[test]
    fn f32_overflow_packs_to_infinity() {
        // 1e40 doesn't fit in f32; the cast saturates to +infinity.
        let data = pack("<f", vec![Value::Float(1e40)]);
        let vals = unpack("<f", &data);
        match &vals[0] {
            Value::Float(f) => {
                k9::assert_equal!(f.is_infinite(), true);
                k9::assert_equal!(f.is_sign_positive(), true);
            }
            other => panic!("expected float, got {other:?}"),
        }
    }

    #[test]
    fn f32_negative_overflow_packs_to_neg_infinity() {
        let data = pack("<f", vec![Value::Float(-1e40)]);
        let vals = unpack("<f", &data);
        match &vals[0] {
            Value::Float(f) => {
                k9::assert_equal!(f.is_infinite(), true);
                k9::assert_equal!(f.is_sign_negative(), true);
            }
            other => panic!("expected float, got {other:?}"),
        }
    }

    #[test]
    fn f64_max_magnitude_roundtrip() {
        let data = pack("<d", vec![Value::Float(f64::MAX)]);
        let vals = unpack("<d", &data);
        k9::assert_equal!(vals[0], Value::Float(f64::MAX));
    }

    #[test]
    fn f64_min_positive_roundtrip() {
        // Smallest positive subnormal.
        let data = pack("<d", vec![Value::Float(f64::MIN_POSITIVE)]);
        let vals = unpack("<d", &data);
        k9::assert_equal!(vals[0], Value::Float(f64::MIN_POSITIVE));
    }

    #[test]
    fn f32_precision_loss_from_f64() {
        // f64(pi) packed as f32 loses precision but is still close.
        let data = pack("<f", vec![Value::Float(std::f64::consts::PI)]);
        // f32(pi) bit pattern is 0x40490FDB — LE: db 0f 49 40.
        k9::assert_equal!(data, hex("db0f4940"));
    }

    // ------------------------------------------------------------------
    // Binary / non-ASCII payload round-trips.
    // ------------------------------------------------------------------

    #[test]
    fn fixed_string_preserves_binary_bytes() {
        let bytes: &[u8] = &[0x00, 0xFF, 0x7F, 0x80, b'\n'];
        let data = pack("c5", vec![Value::String(Bytes::from(bytes))]);
        k9::assert_equal!(&data[..], bytes);
        let vals = unpack("c5", &data);
        k9::assert_equal!(vals[0], Value::String(Bytes::from(bytes)));
    }

    #[test]
    fn len_prefixed_string_preserves_binary_bytes() {
        // `z` can't carry NUL, but `s1` can.
        let bytes: &[u8] = &[0x00, 0x01, 0x02, 0xFF];
        let data = pack("<s1", vec![Value::String(Bytes::from(bytes))]);
        k9::assert_equal!(data[0], 4);
        k9::assert_equal!(&data[1..], bytes);
        let vals = unpack("<s1", &data);
        k9::assert_equal!(vals[0], Value::String(Bytes::from(bytes)));
    }
}
