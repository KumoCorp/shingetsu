//! Lua `string` standard library.
//!
//! Registered as a global `string` table and set as the `__index` of the
//! shared string metatable so that `("hello"):upper()` works.

use bytes::Bytes;

use crate::convert::Variadic;
use crate::error::VmError;
use crate::table::Table;
use crate::value::Value;

/// Clamp a 1-based Lua string index into a 0-based Rust byte offset.
/// Negative indices count from the end.  Out-of-range values are clamped
/// to `[0, len]`.
fn lua_index(i: i64, len: usize) -> usize {
    if i >= 0 {
        ((i as usize).saturating_sub(1)).min(len)
    } else {
        // Negative: -1 == last byte, -len == first byte.
        let pos = len as i64 + i;
        if pos < 0 {
            0
        } else {
            pos as usize
        }
    }
}

/// Same as `lua_index` but the result is *inclusive* (suitable for end
/// positions like the `j` in `string.sub`).
fn lua_index_end(j: i64, len: usize) -> usize {
    if j >= 0 {
        (j as usize).min(len)
    } else {
        let pos = len as i64 + j + 1;
        if pos < 0 {
            0
        } else {
            pos as usize
        }
    }
}

#[crate::module(name = "string")]
pub mod string_mod {
    use super::*;

    // ----------------------------------------------------------------
    // string.len(s)
    // ----------------------------------------------------------------
    #[function]
    fn len(s: Bytes) -> i64 {
        s.len() as i64
    }

    // ----------------------------------------------------------------
    // string.byte(s [, i [, j]])
    // Returns the byte values of s[i] through s[j].
    // ----------------------------------------------------------------
    #[function]
    fn byte(s: Bytes, i: Option<i64>, j: Option<i64>) -> Variadic {
        let len = s.len();
        let i = i.unwrap_or(1);
        let j = j.unwrap_or(i);
        let start = lua_index(i, len);
        let end = lua_index_end(j, len);
        if start >= end {
            return Variadic(vec![]);
        }
        Variadic(
            s[start..end]
                .iter()
                .map(|&b| Value::Integer(b as i64))
                .collect(),
        )
    }

    // ----------------------------------------------------------------
    // string.char(...)
    // Returns a string from the given byte values.
    // ----------------------------------------------------------------
    #[function]
    fn char(args: Variadic) -> Result<Value, VmError> {
        let mut buf = Vec::with_capacity(args.0.len());
        for (i, v) in args.0.iter().enumerate() {
            match v {
                Value::Integer(n) if *n >= 0 && *n <= 255 => buf.push(*n as u8),
                Value::Float(f) if *f >= 0.0 && *f <= 255.0 => buf.push(*f as u8),
                _ => {
                    return Err(VmError::BadArgument {
                        position: i + 1,
                        function: "char".to_owned(),
                        expected: "integer in [0,255]".to_owned(),
                        got: v.type_name().to_owned(),
                    });
                }
            }
        }
        Ok(Value::String(Bytes::from(buf)))
    }

    // ----------------------------------------------------------------
    // string.upper(s)
    // ----------------------------------------------------------------
    #[function]
    fn upper(s: Bytes) -> Bytes {
        Bytes::from(s.to_ascii_uppercase())
    }

    // ----------------------------------------------------------------
    // string.lower(s)
    // ----------------------------------------------------------------
    #[function]
    fn lower(s: Bytes) -> Bytes {
        Bytes::from(s.to_ascii_lowercase())
    }

    // ----------------------------------------------------------------
    // string.reverse(s)
    // ----------------------------------------------------------------
    #[function]
    fn reverse(s: Bytes) -> Bytes {
        let mut v: Vec<u8> = s.to_vec();
        v.reverse();
        Bytes::from(v)
    }

    // ----------------------------------------------------------------
    // string.sub(s, i [, j])
    // ----------------------------------------------------------------
    #[function]
    fn sub(s: Bytes, i: i64, j: Option<i64>) -> Bytes {
        let len = s.len();
        let j = j.unwrap_or(-1);
        let start = lua_index(i, len);
        let end = lua_index_end(j, len);
        if start >= end {
            Bytes::new()
        } else {
            s.slice(start..end)
        }
    }

    // ----------------------------------------------------------------
    // string.rep(s, n [, sep])
    // ----------------------------------------------------------------
    #[function]
    fn rep(s: Bytes, n: i64, sep: Option<Bytes>) -> Bytes {
        if n <= 0 {
            return Bytes::new();
        }
        let n = n as usize;
        let sep = sep.unwrap_or_default();
        let cap = s.len() * n + sep.len() * n.saturating_sub(1);
        let mut buf = Vec::with_capacity(cap);
        for i in 0..n {
            if i > 0 && !sep.is_empty() {
                buf.extend_from_slice(&sep);
            }
            buf.extend_from_slice(&s);
        }
        Bytes::from(buf)
    }
}

/// Build the string library table, register it as the `string` global, and
/// install a string metatable so method-call syntax works on string values.
pub fn register(env: &crate::GlobalEnv) -> Result<(), VmError> {
    let table = string_mod::build_module_table(env)?;

    // Set the string module as a global.
    env.set_global("string", Value::Table(table.clone()));

    // Build a metatable whose __index points to the string table,
    // then install it as the shared string metatable.
    let mt = Table::new();
    mt.raw_set(
        Value::String(Bytes::from_static(b"__index")),
        Value::Table(table),
    )?;
    env.set_string_metatable(mt);

    Ok(())
}
