//! Implementation of the `table` standard library module.

use crate::valuevec;
use shingetsu::Bytes;

use crate::convert::FromLua;
use crate::table::Table;
use crate::value::Value;
use crate::VmError;
use shingetsu_vm::error::VmResultExt;

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
        value: Value::string(msg),
    }
}

/// Argument shapes for `table.insert(t, [pos,] value)`.
#[derive(crate::FromLuaMulti)]
enum InsertArgs {
    AtPos { list: Table, pos: i64, value: Value },
    Append { list: Table, value: Value },
}

/// Operations on Lua tables.
///
/// Most functions in this module work on the *sequence* portion of
/// a table — the contiguous run of integer keys starting at `1`.
/// `t = {10, 20, 30}` has a sequence of length 3; assigning to
/// `t[5]` does not extend the sequence past index 3 because index
/// 4 is missing.  The `#` length operator and the functions in
/// this module work on the sequence.
///
/// A few functions (`table.freeze`, `table.isfrozen`,
/// `table.clone`, etc.) work on the table as a whole, and are
/// noted in their individual documentation.
#[crate::module(name = "table")]
pub mod table_mod {
    use super::*;

    /// Insert a value into a table's sequence.
    ///
    /// With two arguments, appends `value` at the end of the
    /// sequence (at index `#list + 1`).  With three arguments,
    /// inserts `value` at index `pos`, shifting later elements up
    /// by one.
    ///
    /// Raises an error when `pos` is outside `[1, #list + 1]`.
    ///
    /// # Parameters
    ///
    /// - `list` — the table to insert into
    /// - `pos` — 1-based insertion index (3-arg form only)
    /// - `value` — the value to insert
    ///
    /// # Returns
    ///
    /// - nothing
    ///
    /// # Examples
    ///
    /// ```lua
    /// -- Append form.
    /// local t = {10, 20}
    /// table.insert(t, 30)
    /// assert(t[3] == 30)
    /// assert(#t == 3)
    /// ```
    ///
    /// ```lua
    /// -- Insert at a specific position; later elements shift up.
    /// local t = {10, 20, 30}
    /// table.insert(t, 2, 99)
    /// assert(t[1] == 10)
    /// assert(t[2] == 99)
    /// assert(t[3] == 20)
    /// assert(t[4] == 30)
    /// ```
    #[function(variadic)]
    async fn insert(ctx: crate::CallContext, args: InsertArgs) -> Result<(), VmError> {
        // Errors from operating on the table itself (most notably
        // "attempt to modify a readonly table") are attributable to
        // arg 1; tag them so the diagnostic carets point at the
        // table rather than the call.
        match args {
            InsertArgs::Append { list, value } => {
                let len = ctx.table_len(&list).await.with_arg_position(1)?;
                // Shift up and set at len+1, all raw (Lua 5.4 semantics).
                list.raw_set(Value::Integer(len + 1), value)
                    .with_arg_position(1)?;
            }
            InsertArgs::AtPos { list, pos, value } => {
                let len = ctx.table_len(&list).await.with_arg_position(1)?;
                if pos < 1 || pos > len + 1 {
                    return Err(runtime_error(format!(
                        "bad argument #2 to 'insert' (position out of bounds: {} not in [1, {}])",
                        pos,
                        len + 1
                    ))
                    .with_arg_position(2));
                }
                // Shift elements up from len down to pos, then set at pos.
                for i in (pos..=len).rev() {
                    let v = list.raw_get(&Value::Integer(i)).with_arg_position(1)?;
                    list.raw_set(Value::Integer(i + 1), v)
                        .with_arg_position(1)?;
                }
                list.raw_set(Value::Integer(pos), value)
                    .with_arg_position(1)?;
            }
        }

        Ok(())
    }

    /// Remove and return an element from a table's sequence.
    ///
    /// With one argument, removes the last element (index `#t`).
    /// With two arguments, removes the element at `pos`, shifting
    /// later elements down by one.
    ///
    /// Raises an error when `pos` is outside `[1, #t]`, except
    /// that removing from an empty table with no explicit `pos`
    /// returns `nil` without error.
    ///
    /// # Parameters
    ///
    /// - `t` — the table to remove from
    /// - `pos` — optional 1-based index; defaults to `#t`
    ///
    /// # Returns
    ///
    /// - the removed value, or `nil` if the table was empty
    ///
    /// # Examples
    ///
    /// ```lua
    /// -- Remove from the end (default).
    /// local t = {10, 20, 30}
    /// local v = table.remove(t)
    /// assert(v == 30)
    /// assert(#t == 2)
    /// ```
    ///
    /// ```lua
    /// -- Remove at a specific position; later elements shift down.
    /// local t = {10, 20, 30}
    /// local v = table.remove(t, 1)
    /// assert(v == 10)
    /// assert(t[1] == 20)
    /// assert(t[2] == 30)
    /// assert(#t == 2)
    /// ```
    #[function]
    async fn remove(ctx: crate::CallContext, t: Table, pos: Option<i64>) -> Result<Value, VmError> {
        let len = ctx.table_len(&t).await.with_arg_position(1)?;

        let pos = pos.unwrap_or(len);

        if len == 0 && pos == len {
            // Removing from an empty table with no explicit pos returns nil.
            return Ok(Value::Nil);
        }

        if pos < 1 || pos > len {
            return Err(runtime_error(format!(
                "bad argument #2 to 'remove' (position out of bounds: {} not in [1, {}])",
                pos, len
            ))
            .with_arg_position(2));
        }

        // Errors from operating on the table itself (most notably
        // "attempt to modify a readonly table") are attributable to
        // arg 1.
        t.raw_remove(pos as usize).with_arg_position(1)
    }

    /// Join the string representations of a table's sequence.
    ///
    /// Concatenates `t[i]`, `t[i+1]`, …, `t[j]` with `sep` placed
    /// between consecutive elements.  Each element must be a
    /// string or number; any other type raises an error reporting
    /// the offending index.
    ///
    /// # Parameters
    ///
    /// - `t` — the table whose sequence to concatenate
    /// - `sep` — separator string; defaults to `""`
    /// - `i` — starting index; defaults to `1`
    /// - `j` — ending index (inclusive); defaults to `#t`
    ///
    /// # Returns
    ///
    /// - the concatenated string; the empty string when the
    ///   selected range is empty
    ///
    /// # Examples
    ///
    /// ```lua
    /// -- Default separator: empty string.
    /// assert(table.concat({"a", "b", "c"}) == "abc")
    /// ```
    ///
    /// ```lua
    /// -- With a separator.
    /// assert(table.concat({"hello", "world"}, ", ") == "hello, world")
    /// ```
    ///
    /// ```lua
    /// -- Slice a range, mix numbers and strings.
    /// assert(table.concat({1, 2, 3, 4, 5}, "-", 2, 4) == "2-3-4")
    /// ```
    #[function]
    async fn concat(
        ctx: crate::CallContext,
        t: Table,
        sep: Option<Bytes>,
        i: Option<i64>,
        j: Option<i64>,
    ) -> Result<Value, VmError> {
        let len = ctx.table_len(&t).await?;
        let sep = sep.unwrap_or_default();
        let i = i.unwrap_or(1);
        let j = j.unwrap_or(len);

        if i > j {
            return Ok(Value::string(""));
        }

        let mut result = Vec::new();
        for idx in i..=j {
            if idx > i {
                result.extend_from_slice(&sep);
            }
            let key = Value::Integer(idx);
            let val = ctx.table_get(&t, &key).await?;
            match &val {
                Value::String(s) => result.extend_from_slice(s),
                Value::Integer(n) => result.extend_from_slice(n.to_string().as_bytes()),
                Value::Float(f) => result.extend_from_slice(format!("{f}").as_bytes()),
                _ => {
                    return Err(runtime_error(format!(
                        "invalid value ({}) at index {} in table for 'concat'",
                        val.type_name(),
                        idx
                    ))
                    .with_arg_position(1)
                    .with_hint(
                        "`table.concat` requires every element in the \
                         range to be a string or a number; convert other \
                         types via `tostring` first",
                    ));
                }
            }
        }

        Ok(Value::string(result))
    }

    /// Copy a range of elements from one table into another.
    ///
    /// Copies `a1[f]`, `a1[f+1]`, …, `a1[e]` into `a2[t]`,
    /// `a2[t+1]`, ….  The destination defaults to `a1`, so
    /// `table.move(t, 2, 5, 1)` shifts elements within the same
    /// table by reading them all first, which means the source and
    /// destination ranges may overlap safely.
    ///
    /// When `f > e` the function is a no-op and returns `a2`
    /// unchanged.
    ///
    /// # Parameters
    ///
    /// - `a1` — source table
    /// - `f` — first source index
    /// - `e` — last source index (inclusive)
    /// - `t_idx` — destination starting index
    /// - `a2` — destination table; defaults to `a1`
    ///
    /// # Returns
    ///
    /// - the destination table `a2`
    ///
    /// # Examples
    ///
    /// ```lua
    /// -- Copy from one table to another.
    /// local src = {10, 20, 30}
    /// local dst = {}
    /// table.move(src, 1, 3, 1, dst)
    /// assert(dst[1] == 10 and dst[2] == 20 and dst[3] == 30)
    /// ```
    ///
    /// ```lua
    /// -- Shift elements within the same table.
    /// local t = {10, 20, 30, 40, 50}
    /// table.move(t, 1, 3, 3) -- copy t[1..3] to t[3..5]
    /// assert(t[3] == 10 and t[4] == 20 and t[5] == 30)
    /// ```
    #[function(rename = "move")]
    async fn table_move(
        ctx: crate::CallContext,
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

        // Check for overflow: count of elements to move.
        let count = (e as i128) - (f as i128) + 1;
        if count > i64::MAX as i128 {
            return Err(runtime_error(
                "bad argument #3 to 'move' (too many elements to move)".to_owned(),
            )
            .with_arg_position(3));
        }
        // Check for wrap-around in destination range.
        if (t_idx as i128) + count - 1 > i64::MAX as i128 {
            return Err(runtime_error(
                "bad argument #4 to 'move' (destination wrap around)".to_owned(),
            )
            .with_arg_position(4));
        }

        // Collect source values first so overlapping src/dst in the same table
        // works correctly.
        let mut values = Vec::with_capacity((e - f + 1) as usize);
        for i in f..=e {
            values.push(ctx.table_get(&a1, &Value::Integer(i)).await?);
        }

        for (offset, val) in values.into_iter().enumerate() {
            ctx.table_set(&a2, Value::Integer(t_idx + offset as i64), val)
                .await?;
        }

        Ok(Value::Table(a2))
    }

    /// Bundle the function arguments into a new table.
    ///
    /// Returns a new table containing every argument at successive
    /// integer keys (`1`, `2`, …), plus a string field `n` holding
    /// the total argument count.  Unlike `{...}`, `table.pack`
    /// records the argument count even when some of the trailing
    /// arguments are `nil`, which is useful when working with
    /// variadic functions.
    ///
    /// `table.unpack` is the inverse operation.
    ///
    /// # Parameters
    ///
    /// - `...` — zero or more values to pack
    ///
    /// # Returns
    ///
    /// - a new table whose array part is the arguments and whose
    ///   `n` field is the count
    ///
    /// # Examples
    ///
    /// ```lua
    /// local t = table.pack("a", "b", "c")
    /// assert(t.n == 3)
    /// assert(t[1] == "a")
    /// assert(t[3] == "c")
    /// ```
    ///
    /// ```lua
    /// -- The `n` field captures trailing nils that #t would miss.
    /// local t = table.pack(1, nil, 3, nil)
    /// assert(t.n == 4)
    /// ```
    #[function]
    fn pack(args: crate::convert::Variadic) -> Result<Table, VmError> {
        let t = Table::new();
        let n = args.0.len() as i64;
        for (i, v) in args.0.into_iter().enumerate() {
            t.raw_set(Value::Integer(i as i64 + 1), v)?;
        }
        t.raw_set(Value::string("n"), Value::Integer(n))?;
        Ok(t)
    }

    /// Return a range of a table's elements as multiple values.
    ///
    /// Returns `list[i]`, `list[i+1]`, …, `list[j]` as separate
    /// values, suitable for passing to a variadic function or
    /// assigning to multiple variables.  When `i > j` no values
    /// are returned.
    ///
    /// Raises an error when the range would produce more than one
    /// million values.
    ///
    /// `table.pack` is the inverse operation.
    ///
    /// # Parameters
    ///
    /// - `list` — the table to unpack
    /// - `i` — starting index; defaults to `1`
    /// - `j` — ending index (inclusive); defaults to `#list`
    ///
    /// # Returns
    ///
    /// - one value per index in the selected range
    ///
    /// # Examples
    ///
    /// ```lua
    /// -- Pass a table as separate arguments.
    /// local args = {10, 20, 30}
    /// local function sum(a, b, c) return a + b + c end
    /// assert(sum(table.unpack(args)) == 60)
    /// ```
    ///
    /// ```lua
    /// -- Slice a sub-range.
    /// local a, b = table.unpack({"x", "y", "z"}, 2, 3)
    /// assert(a == "y")
    /// assert(b == "z")
    /// ```
    #[function]
    async fn unpack(
        ctx: crate::CallContext,
        t: Table,
        i: Option<i64>,
        j: Option<i64>,
    ) -> Result<crate::convert::Variadic, VmError> {
        let len = ctx.table_len(&t).await?;
        let i = i.unwrap_or(1);
        let j = j.unwrap_or(len);

        if i > j {
            return Ok(crate::convert::Variadic(valuevec![]));
        }

        let count = (j as i128) - (i as i128) + 1;
        // Lua 5.4 limits unpack to LUAI_MAXSTACK (~1000000) results.
        if count > 1_000_000 {
            return Err(
                runtime_error("too many results to unpack".to_owned()).with_hint(
                    "`table.unpack` is capped at 1,000,000 results to avoid \
                 exhausting the call stack; use a `for` loop or split \
                 the table into smaller ranges",
                ),
            );
        }

        let mut result = Vec::with_capacity(count as usize);
        for idx in i..=j {
            result.push(ctx.table_get(&t, &Value::Integer(idx)).await?);
        }

        Ok(crate::convert::Variadic(result.into()))
    }

    /// Create a pre-populated table of fixed size.
    ///
    /// Returns a new table with `count` entries at indices `1`
    /// through `count`, each holding `value` (or `nil` when
    /// omitted, which still pre-allocates capacity for the
    /// entries).  Useful for building an array of known size
    /// without repeatedly resizing the underlying storage.
    ///
    /// Raises an error when `count` is negative.  This is a Luau
    /// extension over Lua 5.4.
    ///
    /// # Parameters
    ///
    /// - `count` — number of entries; must be `>= 0`
    /// - `value` — value to assign to each entry; defaults to `nil`
    ///
    /// # Returns
    ///
    /// - a new table with `count` entries
    ///
    /// # Examples
    ///
    /// ```lua
    /// -- Create an array of zeros.
    /// local zeros = table.create(5, 0)
    /// assert(#zeros == 5)
    /// assert(zeros[1] == 0 and zeros[5] == 0)
    /// ```
    ///
    /// ```lua
    /// -- Pre-allocate without filling.
    /// local t = table.create(3)
    /// assert(t[1] == nil)
    /// ```
    #[function]
    fn create(count: i64, value: Option<Value>) -> Result<Table, VmError> {
        if count < 0 {
            return Err(runtime_error(format!(
                "bad argument #1 to 'create' (size out of range: {count})",
            ))
            .with_arg_position(1)
            .with_hint(
                "`table.create` reserves space for `count` array \
                 entries; the count must be zero or positive",
            ));
        }
        let t = Table::new();
        let value = value.unwrap_or(Value::Nil);
        for i in 1..=count {
            t.raw_set(Value::Integer(i), value.clone())?;
        }
        Ok(t)
    }

    /// Find the first occurrence of a value in a table's sequence.
    ///
    /// Returns the 1-based index of the first element of
    /// `haystack[init]`, `haystack[init+1]`, … that equals
    /// `needle`, or `nil` when no match is found.
    ///
    /// Equality follows Lua's `==`: same type and same value.
    /// Tables and userdata are compared by identity (the same
    /// reference), not content.
    ///
    /// Raises an error when `init < 1`.  This is a Luau extension
    /// over Lua 5.4.
    ///
    /// # Parameters
    ///
    /// - `haystack` — the table to search
    /// - `needle` — the value to find
    /// - `init` — starting index; defaults to `1`
    ///
    /// # Returns
    ///
    /// - the index of the first match, or `nil` when not found
    ///
    /// # Examples
    ///
    /// ```lua
    /// local fruits = {"apple", "banana", "cherry"}
    /// assert(table.find(fruits, "banana") == 2)
    /// assert(table.find(fruits, "durian") == nil)
    /// ```
    ///
    /// ```lua
    /// -- Skip earlier matches with init.
    /// local nums = {1, 2, 3, 2, 1}
    /// assert(table.find(nums, 2) == 2)
    /// assert(table.find(nums, 2, 3) == 4)
    /// ```
    #[function]
    fn find(haystack: Table, needle: Value, init: Option<i64>) -> Result<Option<i64>, VmError> {
        let init = init.unwrap_or(1);
        if init < 1 {
            return Err(runtime_error(format!(
                "bad argument #3 to 'find' (index out of range: {init})",
            ))
            .with_arg_position(3)
            .with_hint(
                "the starting index is 1-based; pass `1` to scan from \
                 the beginning, or omit the argument entirely",
            ));
        }
        let len = haystack.raw_len();
        for i in init..=len {
            if haystack.raw_get(&Value::Integer(i))? == needle {
                return Ok(Some(i));
            }
        }
        Ok(None)
    }

    /// Remove every entry from a table.
    ///
    /// After `table.clear(t)`, `#t` is `0` and every key is
    /// missing.  The table's underlying capacity is preserved, so
    /// re-filling it with similarly-sized data is cheaper than
    /// allocating a fresh table.
    ///
    /// Raises an error when `t` is frozen.  This is a Luau
    /// extension over Lua 5.4.
    ///
    /// # Parameters
    ///
    /// - `t` — the table to clear
    ///
    /// # Returns
    ///
    /// - nothing
    ///
    /// # Examples
    ///
    /// ```lua
    /// local t = {1, 2, 3, key = "value"}
    /// table.clear(t)
    /// assert(#t == 0)
    /// assert(t.key == nil)
    /// ```
    #[function]
    fn clear(t: Table) -> Result<(), VmError> {
        // "attempt to modify a readonly table" is attributable to
        // arg 1.
        t.raw_clear().with_arg_position(1)
    }

    /// Mark a table as read-only.
    ///
    /// After `table.freeze(t)`, every attempt to modify `t` —
    /// assigning a key, calling `table.insert`, etc. — raises
    /// `"attempt to modify a readonly table"`.  Frozen status is
    /// permanent: there is no way to thaw a table.
    ///
    /// `freeze` returns the same table for convenient chaining.
    /// Calling it on an already-frozen table is a no-op.  This is
    /// a Luau extension over Lua 5.4.
    ///
    /// # Parameters
    ///
    /// - `t` — the table to freeze
    ///
    /// # Returns
    ///
    /// - the same table
    ///
    /// # Examples
    ///
    /// ```lua
    /// local config = table.freeze({host = "localhost", port = 8080})
    /// assert(table.isfrozen(config))
    /// local ok = pcall(function() config.port = 9090 end)
    /// assert(not ok)
    /// ```
    #[function]
    fn freeze(t: Table) -> Table {
        t.freeze();
        t
    }

    /// Test whether a table is frozen.
    ///
    /// Returns `true` when `t` has been passed to `table.freeze`,
    /// `false` otherwise.  This is a Luau extension over Lua 5.4.
    ///
    /// # Parameters
    ///
    /// - `t` — the table to test
    ///
    /// # Returns
    ///
    /// - `true` if `t` is frozen, `false` otherwise
    ///
    /// # Examples
    ///
    /// ```lua
    /// local t = {}
    /// assert(not table.isfrozen(t))
    /// table.freeze(t)
    /// assert(table.isfrozen(t))
    /// ```
    #[function]
    fn isfrozen(t: Table) -> bool {
        t.is_frozen()
    }

    /// Make a shallow copy of a table.
    ///
    /// Returns a new table with the same keys, the same values,
    /// and a reference to the same metatable as `t`.  Nested
    /// tables are *not* deep-copied: if `t.inner` is a table, the
    /// clone's `inner` field points at the very same table.
    ///
    /// The clone is never frozen, even when the source is.  This
    /// is a Luau extension over Lua 5.4.
    ///
    /// # Parameters
    ///
    /// - `t` — the table to clone
    ///
    /// # Returns
    ///
    /// - a new (unfrozen) table with the same contents as `t`
    ///
    /// # Examples
    ///
    /// ```lua
    /// local original = {1, 2, 3}
    /// local copy = table.clone(original)
    /// copy[1] = 99
    /// assert(original[1] == 1) -- original is unchanged
    /// assert(copy[1] == 99)
    /// ```
    ///
    /// ```lua
    /// -- Shallow: nested tables are shared, not copied.
    /// local original = {inner = {1, 2}}
    /// local copy = table.clone(original)
    /// copy.inner[1] = 99
    /// assert(original.inner[1] == 99) -- shared inner table
    /// ```
    #[function]
    fn clone(t: Table) -> Table {
        t.raw_clone()
    }

    /// Sort a table's sequence in place.
    ///
    /// With one argument, sorts ascending using the default `<`
    /// operator.  Numeric and string elements compare directly;
    /// tables and userdata fall back to their `__lt` metamethod
    /// when one is defined.  Mixing types that aren't comparable
    /// raises an error.
    ///
    /// With a comparator function `comp`, calls `comp(a, b)` for
    /// each pair under consideration; `comp` should return a
    /// truthy value when `a` should come before `b`.  An
    /// inconsistent comparator (one that says both `a < b` and
    /// `b < a`) raises an error rather than producing a garbage
    /// sort.
    ///
    /// The sort is not guaranteed to be stable.
    ///
    /// # Parameters
    ///
    /// - `t` — the table whose sequence to sort
    /// - `comp` — optional comparator; defaults to `<`
    ///
    /// # Returns
    ///
    /// - nothing
    ///
    /// # Examples
    ///
    /// ```lua
    /// -- Default ascending sort.
    /// local t = {3, 1, 4, 1, 5, 9, 2, 6}
    /// table.sort(t)
    /// assert(table.concat(t, ",") == "1,1,2,3,4,5,6,9")
    /// ```
    ///
    /// ```lua
    /// -- Sort descending with a custom comparator.
    /// local words = {"banana", "apple", "cherry"}
    /// table.sort(words, function(a, b) return a > b end)
    /// assert(words[1] == "cherry")
    /// assert(words[3] == "apple")
    /// ```
    #[function]
    async fn sort(
        ctx: crate::CallContext,
        t: Table,
        comp: Option<crate::Function>,
    ) -> Result<(), VmError> {
        // Use __len to determine the sequence length, then swap the raw
        // array out for in-place sorting.  Element access is raw (Lua 5.4
        // semantics: sort does not invoke __index/__newindex).
        let len = ctx.table_len(&t).await?.max(0) as usize;
        let mut arr = Vec::new();
        t.swap_array(&mut arr)?;

        // Only sort the first `len` elements; preserve the tail.
        let tail = if len < arr.len() {
            arr.split_off(len)
        } else {
            vec![]
        };

        // Trim trailing nils — only sort the non-nil prefix.
        while matches!(arr.last(), Some(Value::Nil)) {
            arr.pop();
        }

        let n = arr.len();
        if n > 1 {
            // Lua 5.4 §6.6: the default sort uses the `<` operator,
            // which dispatches `__lt` on tables and userdata.  Both
            // the user-comparator and default-comparator paths share
            // `async_merge_sort` because either kind of comparison
            // may need to call into the VM.
            let result = async_merge_sort(&mut arr, &ctx, comp.as_ref()).await;
            if let Err(e) = result {
                arr.extend(tail);
                t.swap_array(&mut arr)?;
                // Errors from the comparator ("invalid order function",
                // type errors, etc.) are attributable to the second
                // argument; tag them so the diagnostic carets point
                // at the comparator expression.  When the comparator
                // wasn't supplied (default `<`), the error is about
                // the table elements and we leave it untagged.
                let e = if comp.is_some() {
                    e.with_arg_position(2)
                } else {
                    e
                };
                return Err(e);
            }
        }

        // Reattach any unsorted tail and put the array back.
        arr.extend(tail);
        t.swap_array(&mut arr)?;

        Ok(())
    }
}

/// Async merge sort — O(n log n) comparisons.  When `comp` is `Some`,
/// each comparison invokes the user-supplied Lua function; when
/// `None`, the unified default comparator is used (fast path for
/// numeric/string operands, `__lt` metamethod for tables/userdata).
async fn async_merge_sort(
    arr: &mut [Value],
    ctx: &crate::CallContext,
    comp: Option<&crate::Function>,
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
        let left_first = compare_lt(ctx, comp, &left[i], &right[j]).await?;
        if left_first {
            let reverse_also = compare_lt(ctx, comp, &right[j], &left[i]).await?;
            if reverse_also {
                return Err(
                    runtime_error("invalid order function for sorting".to_owned()).with_hint(
                        "the comparator returned true for both \
                         `cmp(a, b)` and `cmp(b, a)`; it must impose a \
                         strict weak ordering (return true only when its \
                         first argument should sort before its second)",
                    ),
                );
            }
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

/// `a < b` according to either a user-supplied Lua comparator or the
/// Lua 5.4 default `<` operator.  The default path uses the fast
/// numeric/string comparison inline and dispatches `__lt` for
/// tables and userdata.
async fn compare_lt(
    ctx: &crate::CallContext,
    comp: Option<&crate::Function>,
    a: &Value,
    b: &Value,
) -> Result<bool, VmError> {
    if let Some(comp) = comp {
        let result = ctx
            .call_function(comp.clone(), valuevec![a.clone(), b.clone()])
            .await?;
        return Ok(result.first().is_some_and(Value::is_truthy));
    }
    if let Some(b) = default_lt_fast(a, b) {
        return b;
    }
    if let Some(mm) = lookup_lt_metamethod(a, b) {
        let result = ctx
            .call_function(mm, valuevec![a.clone(), b.clone()])
            .await?;
        return Ok(result.first().is_some_and(Value::is_truthy));
    }
    Err(runtime_error(format!(
        "attempt to compare {} with {}",
        a.type_name(),
        b.type_name()
    ))
    .with_hint(
        "the default sort uses `<`, which only compares values of \
         the same numeric or string type; for other types, define a \
         `__lt` metamethod or pass an explicit comparator",
    ))
}

/// Fast-path numeric/string comparison.  Returns `Some(Ok(_))` when
/// both operands are directly comparable; `None` when one of them is
/// a table/userdata and the caller should consult `__lt` instead.
fn default_lt_fast(a: &Value, b: &Value) -> Option<Result<bool, VmError>> {
    Some(match (a, b) {
        (Value::Integer(x), Value::Integer(y)) => Ok(x < y),
        (Value::Float(x), Value::Float(y)) => Ok(x < y),
        (Value::Integer(x), Value::Float(y)) => Ok((*x as f64) < *y),
        (Value::Float(x), Value::Integer(y)) => Ok(*x < (*y as f64)),
        (Value::String(x), Value::String(y)) => Ok(x < y),
        _ => return None,
    })
}

/// Find an `__lt` metamethod on either operand for the default-sort
/// path.  Lua 5.4 §2.5.5: the metamethod is consulted on either
/// operand of the `<` operator.
fn lookup_lt_metamethod(a: &Value, b: &Value) -> Option<crate::Function> {
    let from_value = |v: &Value| match v {
        Value::Table(t) => match t.get_metamethod("__lt") {
            Some(Value::Function(f)) => Some(f),
            _ => None,
        },
        _ => None,
    };
    from_value(a).or_else(|| from_value(b))
}

// =========================================================================
// Registration
// =========================================================================

/// Build the table library table and register it as the `table` global.
pub fn register(env: &crate::GlobalEnv) -> Result<(), VmError> {
    let table = table_mod::build_module_table(env)?;
    env.set_global("table", Value::Table(table));
    env.register_module_type("table", table_mod::module_type());
    Ok(())
}
