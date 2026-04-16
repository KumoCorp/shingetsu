//! Lua `table` standard library.
//!
//! Registered as a global `table` table.  Provides sequential operations
//! (`insert`, `remove`, `concat`), sorting, and packing/unpacking.

use bytes::Bytes;

use crate::convert::{FromLua, Variadic};
use crate::error::VmError;
use crate::table::Table;
use crate::value::Value;

/// Patch a `VmError::BadArgument` with a specific position and function name.
fn patch_arg(e: VmError, position: usize, function: &str) -> VmError {
    match e {
        VmError::BadArgument { expected, got, .. } => VmError::BadArgument {
            position,
            function: function.to_owned(),
            expected,
            got,
        },
        other => other,
    }
}

/// Create a `VmError` for runtime errors.
fn runtime_error(msg: String) -> VmError {
    VmError::LuaError {
        display: msg.clone(),
        value: Value::String(Bytes::from(msg)),
    }
}

/// `table.insert(t, [pos,] value)`
///
/// If `pos` is given, inserts `value` at position `pos`, shifting elements
/// up.  Otherwise appends `value` at the end (`#t + 1`).
fn table_insert(args: Variadic) -> Result<(), VmError> {
    let n = args.0.len();
    if n < 2 {
        return Err(VmError::BadArgument {
            position: if n == 0 { 1 } else { 2 },
            function: "insert".to_owned(),
            expected: "value".to_owned(),
            got: "no value".to_owned(),
        });
    }

    let t = Table::from_lua(args.0[0].clone()).map_err(|e| patch_arg(e, 1, "insert"))?;

    if n == 2 {
        // table.insert(t, value) — append.
        let len = t.raw_len() as usize;
        t.raw_insert(len + 1, args.0[1].clone())?;
    } else {
        // table.insert(t, pos, value)
        let pos = i64::from_lua(args.0[1].clone()).map_err(|e| patch_arg(e, 2, "insert"))?;
        let len = t.raw_len();
        if pos < 1 || pos > len + 1 {
            return Err(runtime_error(format!(
                "bad argument #2 to 'insert' (position out of bounds: {} not in [1, {}])",
                pos,
                len + 1
            )));
        }
        t.raw_insert(pos as usize, args.0[2].clone())?;
    }

    Ok(())
}

#[crate::module(name = "table")]
pub mod table_mod {
    use super::*;

    /// `table.remove(t [, pos])`
    ///
    /// Removes the element at position `pos` (default `#t`), shifting elements
    /// down.  Returns the removed value.
    #[function]
    fn remove(t: Table, pos: Option<i64>) -> Result<Value, VmError> {
        let len = t.raw_len();

        let pos = pos.unwrap_or(len);

        if len == 0 && pos == len {
            // Removing from an empty table with no explicit pos returns nil.
            return Ok(Value::Nil);
        }

        if pos < 1 || pos > len {
            return Err(runtime_error(format!(
                "bad argument #2 to 'remove' (position out of bounds: {} not in [1, {}])",
                pos, len
            )));
        }

        t.raw_remove(pos as usize)
    }

    /// `table.concat(t [, sep [, i [, j]]])`
    ///
    /// Concatenates the string representations of `t[i]` through `t[j]` with
    /// `sep` between them.  Defaults: `sep=""`, `i=1`, `j=#t`.
    #[function]
    fn concat(
        t: Table,
        sep: Option<Bytes>,
        i: Option<i64>,
        j: Option<i64>,
    ) -> Result<Value, VmError> {
        let len = t.raw_len();
        let sep = sep.unwrap_or_default();
        let i = i.unwrap_or(1);
        let j = j.unwrap_or(len);

        if i > j {
            return Ok(Value::String(Bytes::new()));
        }

        let mut result = Vec::new();
        for idx in i..=j {
            if idx > i {
                result.extend_from_slice(&sep);
            }
            let key = Value::Integer(idx);
            let val = t.raw_get(&key)?;
            match &val {
                Value::String(s) => result.extend_from_slice(s),
                Value::Integer(n) => result.extend_from_slice(n.to_string().as_bytes()),
                Value::Float(f) => result.extend_from_slice(format!("{}", f).as_bytes()),
                _ => {
                    return Err(runtime_error(format!(
                        "invalid value ({}) at index {} in table for 'concat'",
                        val.type_name(),
                        idx
                    )));
                }
            }
        }

        Ok(Value::String(Bytes::from(result)))
    }

    /// `table.move(a1, f, e, t [, a2])`
    ///
    /// Copies elements from table `a1` (indices `f` through `e`) into table
    /// `a2` starting at index `t`.  The default for `a2` is `a1`.  The
    /// destination range may overlap with the source range.  Returns `a2`.
    #[function(rename = "move")]
    fn table_move(
        a1: Table,
        f: i64,
        e: i64,
        t_idx: i64,
        a2: Option<Value>,
    ) -> Result<Value, VmError> {
        let a2 = match a2 {
            Some(Value::Nil) | None => a1.clone(),
            Some(v) => Table::from_lua(v).map_err(|e| patch_arg(e, 5, "move"))?,
        };

        if f > e {
            return Ok(Value::Table(a2));
        }

        // Collect source values first so overlapping src/dst in the same table
        // works correctly.
        let values: Vec<Value> = (f..=e)
            .map(|i| a1.raw_get(&Value::Integer(i)))
            .collect::<Result<_, _>>()?;

        for (offset, val) in values.into_iter().enumerate() {
            a2.raw_set(Value::Integer(t_idx + offset as i64), val)?;
        }

        Ok(Value::Table(a2))
    }

    /// `table.pack(...)`
    ///
    /// Returns a new table with all arguments stored in keys 1, 2, ..., plus
    /// a field `"n"` with the total number of arguments.
    #[function]
    fn pack(args: crate::convert::Variadic) -> Result<Value, VmError> {
        let t = Table::new();
        let n = args.0.len() as i64;
        for (i, v) in args.0.into_iter().enumerate() {
            t.raw_set(Value::Integer(i as i64 + 1), v)?;
        }
        t.raw_set(Value::String(Bytes::from_static(b"n")), Value::Integer(n))?;
        Ok(Value::Table(t))
    }

    /// `table.unpack(list [, i [, j]])`
    ///
    /// Returns `list[i], list[i+1], ..., list[j]`.  Defaults: `i=1`, `j=#list`.
    #[function]
    fn unpack(
        t: Table,
        i: Option<i64>,
        j: Option<i64>,
    ) -> Result<crate::convert::Variadic, VmError> {
        let len = t.raw_len();
        let i = i.unwrap_or(1);
        let j = j.unwrap_or(len);

        if i > j {
            return Ok(crate::convert::Variadic(vec![]));
        }

        let mut result = Vec::with_capacity((j - i + 1) as usize);
        for idx in i..=j {
            result.push(t.raw_get(&Value::Integer(idx))?);
        }

        Ok(crate::convert::Variadic(result))
    }

    /// `table.create(count [, value])` (LuaU extension)
    ///
    /// Creates a new table with `count` entries, all set to `value` (or
    /// `nil` if omitted).  `count` must be a non-negative integer.
    #[function]
    fn create(count: i64, value: Option<Value>) -> Result<Value, VmError> {
        if count < 0 {
            return Err(runtime_error(format!(
                "bad argument #1 to 'create' (size out of range: {})",
                count
            )));
        }
        let t = Table::new();
        let value = value.unwrap_or(Value::Nil);
        for i in 1..=count {
            t.raw_set(Value::Integer(i), value.clone())?;
        }
        Ok(Value::Table(t))
    }

    /// `table.find(haystack, needle [, init])` (LuaU extension)
    ///
    /// Returns the index of the first occurrence of `needle` in the array
    /// portion of `haystack`, starting at index `init` (default `1`), or
    /// `nil` if not found.  `init < 1` errors.
    #[function]
    fn find(haystack: Table, needle: Value, init: Option<i64>) -> Result<Value, VmError> {
        let init = init.unwrap_or(1);
        if init < 1 {
            return Err(runtime_error(format!(
                "bad argument #3 to 'find' (index out of range: {})",
                init
            )));
        }
        let len = haystack.raw_len();
        for i in init..=len {
            if haystack.raw_get(&Value::Integer(i))? == needle {
                return Ok(Value::Integer(i));
            }
        }
        Ok(Value::Nil)
    }

    /// `table.clear(t)` (LuaU extension)
    ///
    /// Removes every entry from `t` while preserving its backing capacity.
    /// Errors if `t` is frozen.
    #[function]
    fn clear(t: Table) -> Result<(), VmError> {
        t.raw_clear()
    }

    /// `table.freeze(t)` (LuaU extension)
    ///
    /// Marks `t` as read-only.  Subsequent mutations raise "attempt to
    /// modify a readonly table".  Idempotent; LuaU has no unfreeze.
    /// Returns `t`.
    #[function]
    fn freeze(t: Table) -> Table {
        t.freeze();
        t
    }

    /// `table.isfrozen(t)` (LuaU extension)
    ///
    /// Returns `true` if `t` has been frozen via `table.freeze`.
    #[function]
    fn isfrozen(t: Table) -> bool {
        t.is_frozen()
    }

    /// `table.clone(t)` (LuaU extension)
    ///
    /// Returns a shallow copy of `t`: same keys, values, and metatable
    /// (shared by Arc reference).  The clone is never frozen, even if
    /// `t` is.
    #[function]
    fn clone(t: Table) -> Table {
        t.raw_clone()
    }

    /// `table.sort(t [, comp])`
    ///
    /// Sorts the sequence part of `t` in place.  If `comp` is given it must be
    /// a function that receives two elements and returns `true` when the first
    /// should come before the second.  Otherwise the default `<` order is used.
    ///
    /// Because `comp` may be a Lua function (requiring async dispatch through
    /// the VM), we use a merge sort that `await`s each comparison.
    #[function]
    async fn sort(
        ctx: crate::CallContext,
        t: Table,
        comp: Option<crate::Function>,
    ) -> Result<(), VmError> {
        // Swap the array out of the table so we can sort in place without
        // cloning.  The table's sequence part is temporarily empty; since Lua
        // execution is single-threaded within a Task this is safe.
        let mut arr = Vec::new();
        t.swap_array(&mut arr)?;

        // Trim trailing nils — only sort the non-nil prefix.
        while matches!(arr.last(), Some(Value::Nil)) {
            arr.pop();
        }

        let n = arr.len();
        if n > 1 {
            if let Some(comp) = comp {
                // Lua comparator — merge sort with async comparisons.
                let result = async_merge_sort(&mut arr, &ctx, &comp).await;
                if let Err(e) = result {
                    // Put the (partially sorted) array back before propagating.
                    t.swap_array(&mut arr)?;
                    return Err(e);
                }
            } else {
                // Default `<` order — sort in place synchronously.
                let mut err: Option<VmError> = None;
                arr.sort_by(|a, b| match default_lt(a, b) {
                    Ok(true) => std::cmp::Ordering::Less,
                    Ok(false) => std::cmp::Ordering::Greater,
                    Err(e) => {
                        err.get_or_insert(e);
                        std::cmp::Ordering::Equal
                    }
                });
                if let Some(e) = err {
                    t.swap_array(&mut arr)?;
                    return Err(e);
                }
            }
        }

        // Put the sorted array back.
        t.swap_array(&mut arr)?;

        Ok(())
    }
}

/// Async merge sort — O(n log n) comparisons through a Lua function.
async fn async_merge_sort(
    arr: &mut [Value],
    ctx: &crate::CallContext,
    comp: &crate::Function,
) -> Result<(), VmError> {
    let n = arr.len();
    if n <= 1 {
        return Ok(());
    }
    let mid = n / 2;

    // Recurse on each half.
    // Box::pin to avoid infinite-size future from recursion.
    Box::pin(async_merge_sort(&mut arr[..mid], ctx, comp)).await?;
    Box::pin(async_merge_sort(&mut arr[mid..], ctx, comp)).await?;

    // Merge the two sorted halves into a temporary buffer.
    let left = arr[..mid].to_vec();
    let right = arr[mid..].to_vec();
    let mut i = 0;
    let mut j = 0;
    let mut k = 0;

    while i < left.len() && j < right.len() {
        let result = ctx
            .call_function(comp.clone(), vec![left[i].clone(), right[j].clone()])
            .await?;
        let left_first = match result.first() {
            Some(Value::Boolean(false)) | Some(Value::Nil) | None => false,
            _ => true,
        };
        if left_first {
            arr[k] = left[i].clone();
            i += 1;
        } else {
            arr[k] = right[j].clone();
            j += 1;
        }
        k += 1;
    }

    while i < left.len() {
        arr[k] = left[i].clone();
        i += 1;
        k += 1;
    }
    while j < right.len() {
        arr[k] = right[j].clone();
        j += 1;
        k += 1;
    }

    Ok(())
}

/// Default less-than comparison for `table.sort`.
fn default_lt(a: &Value, b: &Value) -> Result<bool, VmError> {
    match (a, b) {
        (Value::Integer(x), Value::Integer(y)) => Ok(x < y),
        (Value::Float(x), Value::Float(y)) => Ok(x < y),
        (Value::Integer(x), Value::Float(y)) => Ok((*x as f64) < *y),
        (Value::Float(x), Value::Integer(y)) => Ok(*x < (*y as f64)),
        (Value::String(x), Value::String(y)) => Ok(x < y),
        _ => Err(runtime_error(format!(
            "attempt to compare {} with {}",
            a.type_name(),
            b.type_name()
        ))),
    }
}

// =========================================================================
// Registration
// =========================================================================

use crate::function::Function;

/// Build the table library table and register it as the `table` global.
pub fn register(env: &crate::GlobalEnv) -> Result<(), VmError> {
    let table = table_mod::build_module_table(env)?;

    // table.insert stays as a raw handler due to overloaded 2/3-arg arity.
    table.raw_set(
        Value::String(Bytes::from_static(b"insert")),
        Value::Function(Function::wrap("insert", table_insert)),
    )?;

    let unpack = table.raw_get(&Value::String(Bytes::from_static(b"unpack")))?;

    env.set_global("table", Value::Table(table));

    // Also register as global `unpack` for Lua 5.1 compat.
    env.set_global("unpack", unpack);

    Ok(())
}
