mod common;

use common::{run_all, run_err, run_err_rendered, run_one};
use shingetsu_vm::Value;

// table.insert
// ---------------------------------------------------------------------------

#[tokio::test]
async fn table_insert_append() {
    let res = run_all(
        "\
        local t = {1, 2, 3}
        table.insert(t, 4)
        return t[1], t[2], t[3], t[4]",
    )
    .await;
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(1),
            Value::Integer(2),
            Value::Integer(3),
            Value::Integer(4),
        ]
    );
}

#[tokio::test]
async fn table_insert_at_position() {
    let res = run_all(
        "\
        local t = {1, 2, 3}
        table.insert(t, 2, 99)
        return t[1], t[2], t[3], t[4]",
    )
    .await;
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(1),
            Value::Integer(99),
            Value::Integer(2),
            Value::Integer(3),
        ]
    );
}

#[tokio::test]
async fn table_insert_at_beginning() {
    let res = run_all(
        "\
        local t = {10, 20}
        table.insert(t, 1, 5)
        return t[1], t[2], t[3]",
    )
    .await;
    k9::assert_equal!(
        res,
        vec![Value::Integer(5), Value::Integer(10), Value::Integer(20)]
    );
}

#[tokio::test]
async fn table_insert_at_end_with_pos() {
    // Inserting at #t+1 is the same as appending.
    let res = run_all(
        "\
        local t = {1, 2}
        table.insert(t, 3, 99)
        return #t, t[3]",
    )
    .await;
    k9::assert_equal!(res, vec![Value::Integer(3), Value::Integer(99)]);
}

#[tokio::test]
async fn table_insert_updates_length() {
    let res = run_one(
        "\
        local t = {}
        table.insert(t, 'a')
        table.insert(t, 'b')
        return #t",
    )
    .await;
    k9::assert_equal!(res, Value::Integer(2));
}

// ---------------------------------------------------------------------------
// table.remove
// ---------------------------------------------------------------------------

#[tokio::test]
async fn table_remove_last() {
    let res = run_all(
        "\
        local t = {10, 20, 30}
        local v = table.remove(t)
        return v, #t",
    )
    .await;
    k9::assert_equal!(res, vec![Value::Integer(30), Value::Integer(2)]);
}

#[tokio::test]
async fn table_remove_at_position() {
    let res = run_all(
        "\
        local t = {10, 20, 30}
        local v = table.remove(t, 2)
        return v, t[1], t[2], #t",
    )
    .await;
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(20),
            Value::Integer(10),
            Value::Integer(30),
            Value::Integer(2),
        ]
    );
}

#[tokio::test]
async fn table_remove_first() {
    let res = run_all(
        "\
        local t = {'a', 'b', 'c'}
        local v = table.remove(t, 1)
        return v, t[1], t[2]",
    )
    .await;
    k9::assert_equal!(
        res,
        vec![Value::string("a"), Value::string("b"), Value::string("c"),]
    );
}

#[tokio::test]
async fn table_remove_empty() {
    // Removing from an empty table with no pos returns nil.
    let res = run_one(
        "\
        local t = {}
        return table.remove(t)",
    )
    .await;
    k9::assert_equal!(res, Value::Nil);
}

// ---------------------------------------------------------------------------
// table.concat
// ---------------------------------------------------------------------------

#[tokio::test]
async fn table_concat_default_sep() {
    k9::assert_equal!(
        run_one(
            "\
            local t = {'a', 'b', 'c'}
            return table.concat(t)"
        )
        .await,
        Value::string("abc")
    );
}

#[tokio::test]
async fn table_concat_with_sep() {
    k9::assert_equal!(
        run_one(
            "\
            local t = {'hello', 'world'}
            return table.concat(t, ', ')"
        )
        .await,
        Value::string("hello, world")
    );
}

#[tokio::test]
async fn table_concat_range() {
    k9::assert_equal!(
        run_one(
            "\
            local t = {'a', 'b', 'c', 'd', 'e'}
            return table.concat(t, '-', 2, 4)"
        )
        .await,
        Value::string("b-c-d")
    );
}

#[tokio::test]
async fn table_concat_empty_range() {
    // When i > j, the result is an empty string.
    k9::assert_equal!(
        run_one(
            "\
            local t = {'a', 'b'}
            return table.concat(t, ',', 3, 1)"
        )
        .await,
        Value::string("")
    );
}

#[tokio::test]
async fn table_concat_numbers() {
    // Numbers in the sequence are coerced to strings.
    k9::assert_equal!(
        run_one(
            "\
            local t = {1, 2, 3}
            return table.concat(t, '+')"
        )
        .await,
        Value::string("1+2+3")
    );
}

#[tokio::test]
async fn table_concat_empty_table() {
    k9::assert_equal!(run_one("return table.concat({})").await, Value::string(""));
}

#[tokio::test]
async fn table_concat_single_element() {
    k9::assert_equal!(
        run_one(
            "\
            local t = {'only'}
            return table.concat(t, ', ')"
        )
        .await,
        Value::string("only")
    );
}

#[tokio::test]
async fn table_concat_invalid_value() {
    // Non-string, non-number values should error.
    let res = run_one(
        "\
        local ok = pcall(table.concat, {true}, ',')
        return ok",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(false));
}

// ---------------------------------------------------------------------------
// table.insert + table.remove combined
// ---------------------------------------------------------------------------

#[tokio::test]
async fn table_insert_remove_stack() {
    // Use a table as a stack.
    let res = run_all(
        "\
        local t = {}
        table.insert(t, 'a')
        table.insert(t, 'b')
        table.insert(t, 'c')
        local top = table.remove(t)
        return top, #t",
    )
    .await;
    k9::assert_equal!(res, vec![Value::string("c"), Value::Integer(2)]);
}

#[tokio::test]
async fn table_insert_remove_queue() {
    // Use a table as a queue.
    let res = run_all(
        "\
        local t = {}
        table.insert(t, 'a')
        table.insert(t, 'b')
        table.insert(t, 'c')
        local first = table.remove(t, 1)
        return first, t[1], t[2]",
    )
    .await;
    k9::assert_equal!(
        res,
        vec![Value::string("a"), Value::string("b"), Value::string("c"),]
    );
}

// ---------------------------------------------------------------------------
// table.insert — error paths
// ---------------------------------------------------------------------------

#[tokio::test]
async fn table_insert_bad_arg1_type() {
    let res = run_one(
        "\
        local ok = pcall(table.insert, 'notatable', 1)
        return ok",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(false));
}

#[tokio::test]
async fn table_insert_too_few_args_zero() {
    let res = run_one(
        "\
        local ok = pcall(table.insert)
        return ok",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(false));
}

#[tokio::test]
async fn table_insert_too_few_args_one() {
    let res = run_one(
        "\
        local ok = pcall(table.insert, {})
        return ok",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(false));
}

#[tokio::test]
async fn table_insert_pos_out_of_bounds_zero() {
    let res = run_one(
        "\
        local ok = pcall(table.insert, {1, 2}, 0, 99)
        return ok",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(false));
}

#[tokio::test]
async fn table_insert_pos_out_of_bounds_too_large() {
    let res = run_one(
        "\
        local ok = pcall(table.insert, {1, 2}, 100, 99)
        return ok",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(false));
}

// ---------------------------------------------------------------------------
// table.remove — error paths
// ---------------------------------------------------------------------------

#[tokio::test]
async fn table_remove_bad_arg1_type() {
    let res = run_one(
        "\
        local ok = pcall(table.remove, 42)
        return ok",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(false));
}

#[tokio::test]
async fn table_remove_pos_out_of_bounds_zero() {
    let res = run_one(
        "\
        local ok = pcall(table.remove, {1, 2}, 0)
        return ok",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(false));
}

#[tokio::test]
async fn table_remove_pos_out_of_bounds_too_large() {
    let res = run_one(
        "\
        local ok = pcall(table.remove, {1, 2}, 5)
        return ok",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(false));
}

#[tokio::test]
async fn table_remove_returns_string() {
    let res = run_one(
        "\
        local t = {'x', 'y', 'z'}
        return table.remove(t, 2)",
    )
    .await;
    k9::assert_equal!(res, Value::string("y"));
}

// ---------------------------------------------------------------------------
// table.concat — additional coverage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn table_concat_float_values() {
    // Float values in the sequence are coerced to strings.
    k9::assert_equal!(
        run_one(
            "\
            local t = {1.5, 2.5}
            return table.concat(t, '+')"
        )
        .await,
        Value::string("1.5+2.5")
    );
}

#[tokio::test]
async fn table_concat_bad_arg1_type() {
    let res = run_one(
        "\
        local ok = pcall(table.concat, 'notatable')
        return ok",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(false));
}

#[tokio::test]
async fn table_concat_nil_args_use_defaults() {
    // Passing nil for sep, i, j should use defaults.
    k9::assert_equal!(
        run_one(
            "\
            local t = {'a', 'b', 'c'}
            return table.concat(t, nil, nil, nil)"
        )
        .await,
        Value::string("abc")
    );
}

// ---------------------------------------------------------------------------
// table.sort
// ---------------------------------------------------------------------------

#[tokio::test]
async fn table_sort_default() {
    let res = run_all(
        "\
        local t = {3, 1, 4, 1, 5, 9, 2, 6}
        table.sort(t)
        return t[1], t[2], t[3], t[4], t[5], t[6], t[7], t[8]",
    )
    .await;
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(1),
            Value::Integer(1),
            Value::Integer(2),
            Value::Integer(3),
            Value::Integer(4),
            Value::Integer(5),
            Value::Integer(6),
            Value::Integer(9),
        ]
    );
}

#[tokio::test]
async fn table_sort_strings() {
    let res = run_all(
        "\
        local t = {'banana', 'apple', 'cherry'}
        table.sort(t)
        return t[1], t[2], t[3]",
    )
    .await;
    k9::assert_equal!(
        res,
        vec![
            Value::string("apple"),
            Value::string("banana"),
            Value::string("cherry"),
        ]
    );
}

#[tokio::test]
async fn table_sort_custom_comparator() {
    // Sort in descending order.
    let res = run_all(
        "\
        local t = {3, 1, 4, 1, 5}
        table.sort(t, function(a, b) return a > b end)
        return t[1], t[2], t[3], t[4], t[5]",
    )
    .await;
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(5),
            Value::Integer(4),
            Value::Integer(3),
            Value::Integer(1),
            Value::Integer(1),
        ]
    );
}

#[tokio::test]
async fn table_sort_single_element() {
    let res = run_all(
        "\
        local t = {42}
        table.sort(t)
        return t[1]",
    )
    .await;
    k9::assert_equal!(res, vec![Value::Integer(42)]);
}

#[tokio::test]
async fn table_sort_empty() {
    let res = run_one(
        "\
        local t = {}
        table.sort(t)
        return #t",
    )
    .await;
    k9::assert_equal!(res, Value::Integer(0));
}

#[tokio::test]
async fn table_sort_already_sorted() {
    let res = run_all(
        "\
        local t = {1, 2, 3, 4, 5}
        table.sort(t)
        return t[1], t[2], t[3], t[4], t[5]",
    )
    .await;
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(1),
            Value::Integer(2),
            Value::Integer(3),
            Value::Integer(4),
            Value::Integer(5),
        ]
    );
}

#[tokio::test]
async fn table_sort_reverse_sorted() {
    let res = run_all(
        "\
        local t = {5, 4, 3, 2, 1}
        table.sort(t)
        return t[1], t[2], t[3], t[4], t[5]",
    )
    .await;
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(1),
            Value::Integer(2),
            Value::Integer(3),
            Value::Integer(4),
            Value::Integer(5),
        ]
    );
}

#[tokio::test]
async fn table_sort_mixed_int_float() {
    let res = run_all(
        "\
        local t = {3.5, 1, 2.5, 2}
        table.sort(t)
        return t[1], t[2], t[3], t[4]",
    )
    .await;
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(1),
            Value::Integer(2),
            Value::Float(2.5),
            Value::Float(3.5),
        ]
    );
}

#[tokio::test]
async fn table_sort_bad_arg1_type() {
    let res = run_one(
        "\
        local ok = pcall(table.sort, 'notatable')
        return ok",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(false));
}

#[tokio::test]
async fn table_sort_incompatible_types() {
    // Comparing a number with a string should error.
    let res = run_one(
        "\
        local ok = pcall(table.sort, {1, 'a'})
        return ok",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(false));
}

#[tokio::test]
async fn table_sort_custom_comparator_by_field() {
    // Sort a table of records by a field using a comparator.
    let res = run_all(
        "\
        local t = {
            {name='charlie', age=30},
            {name='alice', age=25},
            {name='bob', age=35},
        }
        table.sort(t, function(a, b) return a.age < b.age end)
        return t[1].name, t[2].name, t[3].name",
    )
    .await;
    k9::assert_equal!(
        res,
        vec![
            Value::string("alice"),
            Value::string("charlie"),
            Value::string("bob"),
        ]
    );
}

#[tokio::test]
async fn table_sort_comparator_error_propagates() {
    // If the comparator throws, the error should propagate and the table
    // should still have its elements (not be left empty).
    let res = run_all(
        "\
        local t = {3, 1, 2}
        local ok, msg = pcall(table.sort, t, function(a, b)
            error('comp failed')
        end)
        return ok, #t",
    )
    .await;
    k9::assert_equal!(res, vec![Value::Boolean(false), Value::Integer(3)]);
}

#[tokio::test]
async fn table_sort_comparator_truthy_non_boolean() {
    // A comparator returning a truthy non-boolean (e.g. a number) counts
    // as true.
    let res = run_all(
        "\
        local t = {3, 1, 2}
        table.sort(t, function(a, b) if a < b then return 1 else return nil end end)
        return t[1], t[2], t[3]",
    )
    .await;
    k9::assert_equal!(
        res,
        vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)]
    );
}

#[tokio::test]
async fn table_sort_duplicates_with_comparator() {
    let res = run_all(
        "\
        local t = {5, 3, 3, 1, 5, 1, 2}
        table.sort(t, function(a, b) return a < b end)
        return t[1], t[2], t[3], t[4], t[5], t[6], t[7]",
    )
    .await;
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(1),
            Value::Integer(1),
            Value::Integer(2),
            Value::Integer(3),
            Value::Integer(3),
            Value::Integer(5),
            Value::Integer(5),
        ]
    );
}

#[tokio::test]
async fn table_sort_all_equal() {
    let res = run_all(
        "\
        local t = {7, 7, 7, 7}
        table.sort(t)
        return t[1], t[2], t[3], t[4]",
    )
    .await;
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(7),
            Value::Integer(7),
            Value::Integer(7),
            Value::Integer(7),
        ]
    );
}

#[tokio::test]
async fn table_sort_large_array() {
    // 50 elements to exercise multiple levels of merge sort recursion.
    let res = run_all(
        "\
        local t = {}
        for i = 50, 1, -1 do
            t[#t+1] = i
        end
        table.sort(t)
        return t[1], t[25], t[50]",
    )
    .await;
    k9::assert_equal!(
        res,
        vec![Value::Integer(1), Value::Integer(25), Value::Integer(50)]
    );
}

#[tokio::test]
async fn table_sort_large_array_with_comparator() {
    // 50 elements descending via Lua comparator.
    let res = run_all(
        "\
        local t = {}
        for i = 1, 50 do
            t[#t+1] = i
        end
        table.sort(t, function(a, b) return a > b end)
        return t[1], t[25], t[50]",
    )
    .await;
    k9::assert_equal!(
        res,
        vec![Value::Integer(50), Value::Integer(26), Value::Integer(1)]
    );
}

// ---------------------------------------------------------------------------
#[tokio::test]
async fn table_sort_invalid_order_function() {
    let err = common::run_err_rendered(
        r#"table.sort({3, 1, 2}, function(a, b) return true end)"#,
    )
    .await;
    k9::assert_equal!(
        err,
        r#"error: invalid order function for sorting
 --> test.lua:1:1
  |
1 | table.sort({3, 1, 2}, function(a, b) return true end)
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ invalid order function for sorting
stack traceback:
	test.lua:1: in main chunk"#
    );
}

#[tokio::test]
async fn table_sort_invalid_order_preserves_elements() {
    let res = run_all(
        r#"
        local t = {3, 1, 2}
        local ok = pcall(table.sort, t, function(a, b) return true end)
        return ok, #t
    "#,
    )
    .await;
    k9::assert_equal!(res, vec![Value::Boolean(false), Value::Integer(3)]);
}

// table.move
// ---------------------------------------------------------------------------

#[tokio::test]
async fn table_move_same_table() {
    let res = run_all(
        "\
        local t = {1, 2, 3, 4, 5}
        table.move(t, 1, 3, 2)
        return t[1], t[2], t[3], t[4], t[5]",
    )
    .await;
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(1),
            Value::Integer(1),
            Value::Integer(2),
            Value::Integer(3),
            Value::Integer(5),
        ]
    );
}

#[tokio::test]
async fn table_move_to_other_table() {
    let res = run_all(
        "\
        local src = {10, 20, 30}
        local dst = {0, 0, 0, 0, 0}
        table.move(src, 1, 3, 2, dst)
        return dst[1], dst[2], dst[3], dst[4], dst[5]",
    )
    .await;
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(0),
            Value::Integer(10),
            Value::Integer(20),
            Value::Integer(30),
            Value::Integer(0),
        ]
    );
}

#[tokio::test]
async fn table_move_returns_destination() {
    let res = run_one(
        "\
        local src = {1, 2, 3}
        local dst = {}
        local r = table.move(src, 1, 3, 1, dst)
        return r == dst",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(true));
}

#[tokio::test]
async fn table_move_empty_range() {
    // f > e means nothing is copied.
    let res = run_one(
        "\
        local t = {1, 2, 3}
        table.move(t, 3, 1, 1)
        return t[1]",
    )
    .await;
    k9::assert_equal!(res, Value::Integer(1));
}

#[tokio::test]
async fn table_move_bad_arg1_type() {
    let res = run_one(
        "\
        local ok = pcall(table.move, 'notatable', 1, 2, 1)
        return ok",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(false));
}

// ---------------------------------------------------------------------------
// table.pack
// ---------------------------------------------------------------------------

#[tokio::test]
async fn table_pack_basic() {
    let res = run_all(
        "\
        local t = table.pack(10, 20, 30)
        return t[1], t[2], t[3], t.n",
    )
    .await;
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(10),
            Value::Integer(20),
            Value::Integer(30),
            Value::Integer(3),
        ]
    );
}

#[tokio::test]
async fn table_pack_empty() {
    let res = run_one(
        "\
        local t = table.pack()
        return t.n",
    )
    .await;
    k9::assert_equal!(res, Value::Integer(0));
}

#[tokio::test]
async fn table_pack_with_nils() {
    // Nils in the middle are preserved; n reflects total count.
    let res = run_all(
        "\
        local t = table.pack(1, nil, 3)
        return t.n, t[1], t[2], t[3]",
    )
    .await;
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(3),
            Value::Integer(1),
            Value::Nil,
            Value::Integer(3),
        ]
    );
}

// ---------------------------------------------------------------------------
// table.unpack
// ---------------------------------------------------------------------------

#[tokio::test]
async fn table_unpack_basic() {
    let res = run_all(
        "\
        return table.unpack({10, 20, 30})",
    )
    .await;
    k9::assert_equal!(
        res,
        vec![Value::Integer(10), Value::Integer(20), Value::Integer(30)]
    );
}

#[tokio::test]
async fn table_unpack_range() {
    let res = run_all(
        "\
        return table.unpack({10, 20, 30, 40, 50}, 2, 4)",
    )
    .await;
    k9::assert_equal!(
        res,
        vec![Value::Integer(20), Value::Integer(30), Value::Integer(40)]
    );
}

#[tokio::test]
async fn table_unpack_empty_range() {
    // i > j returns nothing.
    let res = run_all(
        "\
        return table.unpack({1, 2, 3}, 3, 1)",
    )
    .await;
    k9::assert_equal!(res, vec![]);
}

#[tokio::test]
async fn table_unpack_single() {
    let res = run_all(
        "\
        return table.unpack({99}, 1, 1)",
    )
    .await;
    k9::assert_equal!(res, vec![Value::Integer(99)]);
}

#[tokio::test]
async fn table_unpack_bad_arg1_type() {
    let res = run_one(
        "\
        local ok = pcall(table.unpack, 'notatable')
        return ok",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(false));
}

// ---------------------------------------------------------------------------
// global unpack (Lua 5.1 compat)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn global_unpack_basic() {
    let res = run_all(
        "\
        return unpack({10, 20, 30})",
    )
    .await;
    k9::assert_equal!(
        res,
        vec![Value::Integer(10), Value::Integer(20), Value::Integer(30)]
    );
}

#[tokio::test]
async fn global_unpack_range() {
    let res = run_all(
        "\
        return unpack({'a', 'b', 'c', 'd'}, 2, 3)",
    )
    .await;
    k9::assert_equal!(res, vec![Value::string("b"), Value::string("c"),]);
}

// ---------------------------------------------------------------------------
// table.move — additional coverage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn table_move_too_few_args() {
    // Only 3 args instead of the required 4.
    let res = run_one(
        "\
        local ok = pcall(table.move, {1,2,3}, 1, 2)
        return ok",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(false));
}

#[tokio::test]
async fn table_move_bad_a2_type() {
    let res = run_one(
        "\
        local ok = pcall(table.move, {1,2,3}, 1, 3, 1, 'notatable')
        return ok",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(false));
}

#[tokio::test]
async fn table_move_overlap_shift_left() {
    // Copy elements 3..5 to starting at index 1 (shift left within same table).
    let res = run_all(
        "\
        local t = {10, 20, 30, 40, 50}
        table.move(t, 3, 5, 1)
        return t[1], t[2], t[3], t[4], t[5]",
    )
    .await;
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(30),
            Value::Integer(40),
            Value::Integer(50),
            Value::Integer(40),
            Value::Integer(50),
        ]
    );
}

// ---------------------------------------------------------------------------
// table.pack — additional coverage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn table_pack_mixed_types() {
    let res = run_all(
        "\
        local t = table.pack(1, 'hello', true, nil, 3.14)
        return t.n, t[1], t[2], t[3], t[5]",
    )
    .await;
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(5),
            Value::Integer(1),
            Value::string("hello"),
            Value::Boolean(true),
            Value::Float(3.14),
        ]
    );
}

#[tokio::test]
async fn table_pack_unpack_roundtrip() {
    let res = run_all(
        "\
        local a, b, c = table.unpack(table.pack(10, 20, 30))
        return a, b, c",
    )
    .await;
    k9::assert_equal!(
        res,
        vec![Value::Integer(10), Value::Integer(20), Value::Integer(30)]
    );
}

// ---------------------------------------------------------------------------
// table.unpack — additional coverage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn table_unpack_nils_in_middle() {
    // Gaps in the table come back as nil.
    let res = run_all(
        "\
        local t = {1, nil, 3}
        return table.unpack(t, 1, 3)",
    )
    .await;
    k9::assert_equal!(res, vec![Value::Integer(1), Value::Nil, Value::Integer(3)]);
}

#[tokio::test]
async fn table_unpack_explicit_i_only() {
    // Only i specified; j defaults to #t.
    let res = run_all(
        "\
        return table.unpack({10, 20, 30, 40}, 3)",
    )
    .await;
    k9::assert_equal!(res, vec![Value::Integer(30), Value::Integer(40)]);
}

#[tokio::test]
async fn table_unpack_nil_args_use_defaults() {
    let res = run_all(
        "\
        return table.unpack({10, 20, 30}, nil, nil)",
    )
    .await;
    k9::assert_equal!(
        res,
        vec![Value::Integer(10), Value::Integer(20), Value::Integer(30)]
    );
}

// ===========================================================================
// table.insert — argument count validation
// ===========================================================================

#[tokio::test]
async fn table_insert_too_many_args() {
    k9::assert_equal!(
        run_err_rendered("table.insert({}, 2, 3, 4)").await,
        "\
error: bad argument to 'insert' (expected at most 3 arguments but got 4)
 --> test.lua:1:1
  |
1 | table.insert({}, 2, 3, 4)
  | ^^^^^^^^^^^^^^^^^^^^^^^^^ bad argument to 'insert' (expected at most 3 arguments but got 4)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn table_insert_too_few_args() {
    k9::assert_equal!(
        run_err_rendered("table.insert({})").await,
        "\
error: bad argument to 'insert' (expected at least 2 arguments but got 1)
 --> test.lua:1:1
  |
1 | table.insert({})
  | ^^^^^^^^^^^^^^^^ bad argument to 'insert' (expected at least 2 arguments but got 1)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn table_insert_no_args() {
    k9::assert_equal!(
        run_err_rendered("table.insert()").await,
        "\
error: bad argument to 'insert' (expected at least 2 arguments but got 0)
 --> test.lua:1:1
  |
1 | table.insert()
  | ^^^^^^^^^^^^^^ bad argument to 'insert' (expected at least 2 arguments but got 0)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn table_insert_bad_pos_type() {
    k9::assert_equal!(
        run_err_rendered(r#"table.insert({1,2}, "hello", "world")"#).await,
        "\
error: bad argument #2 to 'insert' (number expected, got string)
 --> test.lua:1:1
  |
1 | table.insert({1,2}, \"hello\", \"world\")
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ bad argument #2 to 'insert' (number expected, got string)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

// ===========================================================================
// table.move — overflow / wrap-around validation
// ===========================================================================

#[tokio::test]
async fn table_move_too_many_elements() {
    k9::assert_equal!(
        run_err("table.move({}, 0, math.maxinteger, 1)").await,
        "bad argument #3 to 'move' (too many elements to move)"
    );
}

#[tokio::test]
async fn table_move_destination_wrap_around() {
    k9::assert_equal!(
        run_err("table.move({}, 1, math.maxinteger, 2)").await,
        "bad argument #4 to 'move' (destination wrap around)"
    );
}

#[tokio::test]
async fn table_move_small_range_still_works() {
    let res = run_all(
        "\
        local t = table.move({10, 20, 30}, 1, 3, 2)
        return t[1], t[2], t[3], t[4]",
    )
    .await;
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(10),
            Value::Integer(10),
            Value::Integer(20),
            Value::Integer(30),
        ]
    );
}

// ===========================================================================
// table.unpack — "too many results" validation
// ===========================================================================

#[tokio::test]
async fn table_unpack_too_many_results() {
    k9::assert_equal!(
        run_err("return table.unpack({}, 1, math.maxinteger)").await,
        "too many results to unpack"
    );
}

// ===========================================================================
// Inline table constructor as function argument
// ===========================================================================

#[tokio::test]
async fn call_with_table_constructor_arg() {
    let res = run_one(
        "\
        local function id(x) return x end
        local r = id{10, 20, 30}
        return r[2]",
    )
    .await;
    k9::assert_equal!(res, Value::Integer(20));
}

#[tokio::test]
async fn call_with_table_constructor_nested_in_call() {
    let res = run_all(
        "\
        local function id(x) return x end
        local function f(a, b) return a, b[1] end
        return f(1, id{42})",
    )
    .await;
    k9::assert_equal!(res, vec![Value::Integer(1), Value::Integer(42)]);
}

// ===========================================================================
// math library
// ===========================================================================

// ---------------------------------------------------------------------------
