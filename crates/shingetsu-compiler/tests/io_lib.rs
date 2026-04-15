mod common;

use bytes::Bytes;
use shingetsu_compiler::{compile, CompileOptions};
use shingetsu_vm::{Function, GlobalEnv, Task, Value};

// ===========================================================================
// Helpers
// ===========================================================================

/// Create an environment with builtins + io library registered.
fn io_env() -> GlobalEnv {
    let env = GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("register builtins");
    shingetsu::io_lib::register(&env).expect("register io");
    env
}

/// Run Lua code with io library available, return all values.
fn run_io(src: &str) -> Vec<Value> {
    let opts = CompileOptions::default();
    let bc = compile(src, &opts).expect("compile");
    let env = io_env();
    let func = Function::lua(bc.top_level, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("rt");
    rt.block_on(Task::new(env, func, vec![])).expect("run")
}

/// Run Lua code with io library available, return first value.
fn run_io_one(src: &str) -> Value {
    run_io(src).into_iter().next().unwrap_or(Value::Nil)
}

/// Run Lua code with io library available, expect an error.
fn run_io_err(src: &str) -> String {
    let opts = CompileOptions::default();
    let bc = compile(src, &opts).expect("compile");
    let env = io_env();
    let func = Function::lua(bc.top_level, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("rt");
    rt.block_on(Task::new(env, func, vec![]))
        .unwrap_err()
        .to_string()
}

/// Create a temp file with given contents, return its path as a String.
fn temp_file(contents: &[u8]) -> (tempfile::NamedTempFile, String) {
    use std::io::Write;
    let mut tmp = tempfile::NamedTempFile::new().expect("create temp");
    tmp.write_all(contents).expect("write");
    tmp.flush().expect("flush");
    let path = tmp.path().to_str().expect("path").to_owned();
    (tmp, path)
}

/// Create an empty temp dir, return the TempDir guard and a path to a
/// file inside it (the file does not exist yet).
fn temp_dir_file(name: &str) -> (tempfile::TempDir, String) {
    let dir = tempfile::TempDir::new().expect("create dir");
    let path = dir.path().join(name).to_str().expect("path").to_owned();
    (dir, path)
}

// ===========================================================================
// io.open — read mode
// ===========================================================================

#[test]
fn io_open_read_all() {
    let (_tmp, path) = temp_file(b"hello world");
    let result = run_io(&format!(
        r#"
        local f = io.open("{path}", "r")
        return f:read("*a")
        "#
    ));
    k9::assert_equal!(result, vec![Value::String(Bytes::from("hello world"))]);
}

#[test]
fn io_open_read_line() {
    let (_tmp, path) = temp_file(b"line1\nline2\nline3");
    let result = run_io(&format!(
        r#"
        local f = io.open("{path}", "r")
        local a = f:read("*l")
        local b = f:read("*l")
        local c = f:read("*l")
        f:close()
        return a, b, c
        "#
    ));
    k9::assert_equal!(
        result,
        vec![
            Value::String(Bytes::from("line1")),
            Value::String(Bytes::from("line2")),
            Value::String(Bytes::from("line3")),
        ]
    );
}

#[test]
fn io_open_read_number() {
    let (_tmp, path) = temp_file(b"  42.5  99  ");
    let result = run_io(&format!(
        r#"
        local f = io.open("{path}", "r")
        local a = f:read("*n")
        local b = f:read("*n")
        f:close()
        return a, b
        "#
    ));
    k9::assert_equal!(result, vec![Value::Float(42.5), Value::Float(99.0)]);
}

#[test]
fn io_open_read_bytes() {
    let (_tmp, path) = temp_file(b"abcdefghij");
    let result = run_io(&format!(
        r#"
        local f = io.open("{path}", "r")
        local a = f:read(3)
        local b = f:read(4)
        f:close()
        return a, b
        "#
    ));
    k9::assert_equal!(
        result,
        vec![
            Value::String(Bytes::from("abc")),
            Value::String(Bytes::from("defg")),
        ]
    );
}

#[test]
fn io_open_read_at_eof() {
    let (_tmp, path) = temp_file(b"short");
    let result = run_io_one(&format!(
        r#"
        local f = io.open("{path}", "r")
        f:read("*a")  -- consume all
        return f:read("*l")  -- should be nil at EOF
        "#
    ));
    k9::assert_equal!(result, Value::Nil);
}

// ===========================================================================
// io.open — write mode
// ===========================================================================

#[test]
fn io_open_write_and_read_back() {
    let (_dir, path) = temp_dir_file("output.txt");
    run_io(&format!(
        r#"
        local f = io.open("{path}", "w")
        f:write("hello ")
        f:write("world")
        f:close()
        "#
    ));
    let contents = std::fs::read(&path).expect("read back");
    k9::assert_equal!(contents.as_slice(), b"hello world");
}

#[test]
fn io_open_write_numbers() {
    let (_dir, path) = temp_dir_file("numbers.txt");
    run_io(&format!(
        r#"
        local f = io.open("{path}", "w")
        f:write(42)
        f:write(" ")
        f:write(3.14)
        f:close()
        "#
    ));
    let contents = std::fs::read(&path).expect("read back");
    k9::assert_equal!(contents.as_slice(), b"42 3.14");
}

#[test]
fn io_open_write_chaining() {
    let (_dir, path) = temp_dir_file("chain.txt");
    run_io(&format!(
        r#"
        local f = io.open("{path}", "w")
        f = f:write("a")
        f = f:write("b")
        f:write("c")
        f:close()
        "#
    ));
    let contents = std::fs::read(&path).expect("read back");
    k9::assert_equal!(contents.as_slice(), b"abc");
}

// ===========================================================================
// io.open — append mode
// ===========================================================================

#[test]
fn io_open_append() {
    let (_tmp, path) = temp_file(b"existing ");
    run_io(&format!(
        r#"
        local f = io.open("{path}", "a")
        f:write("appended")
        f:close()
        "#
    ));
    let contents = std::fs::read(&path).expect("read back");
    k9::assert_equal!(contents.as_slice(), b"existing appended");
}

// ===========================================================================
// io.open — read+write mode
// ===========================================================================

#[test]
fn io_open_read_write_mode() {
    let (_tmp, path) = temp_file(b"hello world");
    let result = run_io(&format!(
        r#"
        local f = io.open("{path}", "r+")
        local head = f:read(5)
        f:seek("set", 6)
        f:write("lua!!")
        f:close()
        return head
        "#
    ));
    k9::assert_equal!(result, vec![Value::String(Bytes::from("hello"))]);
    let contents = std::fs::read(&path).expect("read back");
    k9::assert_equal!(contents.as_slice(), b"hello lua!!");
}

// ===========================================================================
// io.open — error cases
// ===========================================================================

#[test]
fn io_open_nonexistent_returns_nil() {
    let result = run_io(
        r#"
        local f, err = io.open("/tmp/nonexistent_shingetsu_xyz_42", "r")
        return f, type(err)
        "#,
    );
    k9::assert_equal!(result[0], Value::Nil);
    k9::assert_equal!(result[1], Value::String(Bytes::from("string")));
}

#[test]
fn io_open_default_mode_is_read() {
    let (_tmp, path) = temp_file(b"default mode");
    let result = run_io_one(&format!(
        r#"
        local f = io.open("{path}")
        return f:read("*a")
        "#
    ));
    k9::assert_equal!(result, Value::String(Bytes::from("default mode")));
}

// ===========================================================================
// io.close
// ===========================================================================

#[test]
fn io_close_file() {
    let (_tmp, path) = temp_file(b"data");
    let result = run_io(&format!(
        r#"
        local f = io.open("{path}", "r")
        io.close(f)
        return io.type(f)
        "#
    ));
    k9::assert_equal!(result, vec![Value::String(Bytes::from("closed file"))]);
}

// ===========================================================================
// io.type
// ===========================================================================

#[test]
fn io_type_open_file() {
    let (_tmp, path) = temp_file(b"");
    let result = run_io_one(&format!(
        r#"
        local f = io.open("{path}", "r")
        return io.type(f)
        "#
    ));
    k9::assert_equal!(result, Value::String(Bytes::from("file")));
}

#[test]
fn io_type_closed_file() {
    let (_tmp, path) = temp_file(b"");
    let result = run_io_one(&format!(
        r#"
        local f = io.open("{path}", "r")
        f:close()
        return io.type(f)
        "#
    ));
    k9::assert_equal!(result, Value::String(Bytes::from("closed file")));
}

#[test]
fn io_type_non_file() {
    let result = run_io(
        r#"
        return io.type(42), io.type("hello"), io.type(nil), io.type(true)
        "#,
    );
    k9::assert_equal!(result, vec![Value::Nil, Value::Nil, Value::Nil, Value::Nil]);
}

// ===========================================================================
// io.tmpfile
// ===========================================================================

#[test]
fn io_tmpfile_write_and_read() {
    let result = run_io_one(
        r#"
        local f = io.tmpfile()
        f:write("temp data")
        f:seek("set", 0)
        return f:read("*a")
        "#,
    );
    k9::assert_equal!(result, Value::String(Bytes::from("temp data")));
}

#[test]
fn io_tmpfile_is_file_type() {
    let result = run_io_one(
        r#"
        local f = io.tmpfile()
        return io.type(f)
        "#,
    );
    k9::assert_equal!(result, Value::String(Bytes::from("file")));
}

// ===========================================================================
// f:seek
// ===========================================================================

#[test]
fn file_seek_set_cur_end() {
    let (_tmp, path) = temp_file(b"abcdefghij");
    let result = run_io(&format!(
        r#"
        local f = io.open("{path}", "r")
        local pos1 = f:seek("set", 3)
        local ch = f:read(1)
        local pos2 = f:seek("cur", 0)
        local pos3 = f:seek("end", -2)
        local tail = f:read("*a")
        f:close()
        return pos1, ch, pos2, pos3, tail
        "#
    ));
    k9::assert_equal!(
        result,
        vec![
            Value::Integer(3),
            Value::String(Bytes::from("d")),
            Value::Integer(4),
            Value::Integer(8),
            Value::String(Bytes::from("ij")),
        ]
    );
}

// ===========================================================================
// f:flush
// ===========================================================================

#[test]
fn file_flush() {
    let (_dir, path) = temp_dir_file("flush.txt");
    run_io(&format!(
        r#"
        local f = io.open("{path}", "w")
        f:write("flushed")
        f:flush()
        "#
    ));
    // After flush, data should be on disk even without close.
    let contents = std::fs::read(&path).expect("read back");
    k9::assert_equal!(contents.as_slice(), b"flushed");
}

// ===========================================================================
// f:lines
// ===========================================================================

#[test]
fn file_lines_iterator() {
    let (_tmp, path) = temp_file(b"alpha\nbeta\ngamma");
    let result = run_io(&format!(
        r#"
        local f = io.open("{path}", "r")
        local lines = {{}}
        for line in f:lines() do
            lines[#lines + 1] = line
        end
        f:close()
        return lines[1], lines[2], lines[3], #lines
        "#
    ));
    k9::assert_equal!(
        result,
        vec![
            Value::String(Bytes::from("alpha")),
            Value::String(Bytes::from("beta")),
            Value::String(Bytes::from("gamma")),
            Value::Integer(3),
        ]
    );
}

// ===========================================================================
// f:setvbuf
// ===========================================================================

#[test]
fn file_setvbuf_no() {
    let (_dir, path) = temp_dir_file("setvbuf.txt");
    run_io(&format!(
        r#"
        local f = io.open("{path}", "w")
        f:setvbuf("no")
        f:write("immediate")
        -- In unbuffered mode, data should be on disk without flush.
        "#
    ));
    let contents = std::fs::read(&path).expect("read back");
    k9::assert_equal!(contents.as_slice(), b"immediate");
}

// ===========================================================================
// Operations on closed files
// ===========================================================================

#[test]
fn closed_file_read_returns_nil_and_error() {
    let (_tmp, path) = temp_file(b"data");
    let result = run_io(&format!(
        r#"
        local f = io.open("{path}", "r")
        f:close()
        local val, err = f:read("*a")
        return val, err
        "#
    ));
    k9::assert_equal!(result[0], Value::Nil);
    k9::assert_equal!(
        result[1],
        Value::String(Bytes::from("attempt to use a closed file"))
    );
}

#[test]
fn closed_file_write_returns_nil_and_error() {
    let (_dir, path) = temp_dir_file("closed.txt");
    let result = run_io(&format!(
        r#"
        local f = io.open("{path}", "w")
        f:close()
        local val, err = f:write("data")
        return val, err
        "#
    ));
    k9::assert_equal!(result[0], Value::Nil);
    k9::assert_equal!(
        result[1],
        Value::String(Bytes::from("attempt to use a closed file"))
    );
}

// ===========================================================================
// f:read with multiple format args
// ===========================================================================

#[test]
fn read_multiple_formats() {
    let (_tmp, path) = temp_file(b"42 hello\nworld");
    let result = run_io(&format!(
        r#"
        local f = io.open("{path}", "r")
        local n, line, rest = f:read("*n", "*l", "*a")
        f:close()
        return n, line, rest
        "#
    ));
    k9::assert_equal!(
        result,
        vec![
            Value::Float(42.0),
            Value::String(Bytes::from(" hello")),
            Value::String(Bytes::from("world")),
        ]
    );
}

// ===========================================================================
// __tostring metamethod
// ===========================================================================

#[test]
fn file_tostring() {
    let (_tmp, path) = temp_file(b"");
    let result = run_io_one(&format!(
        r#"
        local f = io.open("{path}", "r")
        return tostring(f)
        "#
    ));
    // Should be "file (<path>)" — just check it starts with "file ("
    match &result {
        Value::String(s) => {
            let s = String::from_utf8_lossy(s);
            assert!(s.starts_with("file ("), "got: {s}");
        }
        other => panic!("expected string, got {other:?}"),
    }
}

#[test]
fn closed_file_tostring() {
    let (_tmp, path) = temp_file(b"");
    let result = run_io_one(&format!(
        r#"
        local f = io.open("{path}", "r")
        f:close()
        return tostring(f)
        "#
    ));
    k9::assert_equal!(result, Value::String(Bytes::from("file (closed)")));
}

// ===========================================================================
// io.open — invalid mode
// ===========================================================================

#[test]
fn io_open_invalid_mode() {
    let (_tmp, path) = temp_file(b"");
    let err = run_io_err(&format!(r#"io.open("{path}", "x")"#));
    assert!(err.contains("invalid mode"), "got: {err}");
}

// ===========================================================================
// f:read("*L") — keep newline
// ===========================================================================

#[test]
fn read_keep_newline() {
    let (_tmp, path) = temp_file(b"line1\nline2\n");
    let result = run_io(&format!(
        r#"
        local f = io.open("{path}", "r")
        local a = f:read("*L")
        local b = f:read("*L")
        f:close()
        return a, b
        "#
    ));
    k9::assert_equal!(
        result,
        vec![
            Value::String(Bytes::from("line1\n")),
            Value::String(Bytes::from("line2\n")),
        ]
    );
}

// ===========================================================================
// f:lines with format arg
// ===========================================================================

#[test]
fn file_lines_with_number_format() {
    let (_tmp, path) = temp_file(b"10\n20\n30\n");
    let result = run_io(&format!(
        r#"
        local f = io.open("{path}", "r")
        local nums = {{}}
        for n in f:lines("*n") do
            nums[#nums + 1] = n
        end
        f:close()
        return nums[1], nums[2], nums[3], #nums
        "#
    ));
    k9::assert_equal!(
        result,
        vec![
            Value::Float(10.0),
            Value::Float(20.0),
            Value::Float(30.0),
            Value::Integer(3),
        ]
    );
}

// ===========================================================================
// f:setvbuf through Lua
// ===========================================================================

#[test]
fn file_setvbuf_full() {
    let (_dir, path) = temp_dir_file("setvbuf_full.txt");
    run_io(&format!(
        r#"
        local f = io.open("{path}", "w")
        f:setvbuf("full")
        f:write("buffered")
        f:flush()
        f:close()
        "#
    ));
    let contents = std::fs::read(&path).expect("read back");
    k9::assert_equal!(contents.as_slice(), b"buffered");
}

#[test]
fn file_setvbuf_line() {
    let (_dir, path) = temp_dir_file("setvbuf_line.txt");
    run_io(&format!(
        r#"
        local f = io.open("{path}", "w")
        f:setvbuf("line")
        f:write("line buffered\n")
        f:close()
        "#
    ));
    let contents = std::fs::read(&path).expect("read back");
    k9::assert_equal!(contents.as_slice(), b"line buffered\n");
}

// ===========================================================================
// f:seek() with no args — defaults to "cur", 0
// ===========================================================================

#[test]
fn file_seek_default_args() {
    let (_tmp, path) = temp_file(b"abcdef");
    let result = run_io(&format!(
        r#"
        local f = io.open("{path}", "r")
        f:read(3)
        local pos = f:seek()
        f:close()
        return pos
        "#
    ));
    k9::assert_equal!(result, vec![Value::Integer(3)]);
}

// ===========================================================================
// io.close on already-closed file
// ===========================================================================

#[test]
fn io_close_already_closed() {
    let (_tmp, path) = temp_file(b"");
    let result = run_io(&format!(
        r#"
        local f = io.open("{path}", "r")
        f:close()
        local ok, err = io.close(f)
        return ok, err
        "#
    ));
    k9::assert_equal!(result[0], Value::Nil);
    k9::assert_equal!(
        result[1],
        Value::String(Bytes::from("attempt to use a closed file"))
    );
}

// ===========================================================================
// Write + read round trip in one script
// ===========================================================================

#[test]
fn write_then_read_round_trip() {
    let (_dir, path) = temp_dir_file("roundtrip.txt");
    let result = run_io_one(&format!(
        r#"
        local f = io.open("{path}", "w")
        f:write("round trip")
        f:close()
        local f2 = io.open("{path}", "r")
        local data = f2:read("*a")
        f2:close()
        return data
        "#
    ));
    k9::assert_equal!(result, Value::String(Bytes::from("round trip")));
}

// ===========================================================================
// Binary data (non-UTF8)
// ===========================================================================

#[test]
fn binary_data_round_trip() {
    let data: Vec<u8> = (0..=255).collect();
    let (_tmp, path) = temp_file(&data);
    let result = run_io_one(&format!(
        r#"
        local f = io.open("{path}", "rb")
        return f:read("*a")
        "#
    ));
    match &result {
        Value::String(s) => {
            k9::assert_equal!(s.len(), 256);
        }
        other => panic!("expected string, got {other:?}"),
    }
}

// ===========================================================================
// Multiple files open simultaneously
// ===========================================================================

#[test]
fn multiple_files_open() {
    let (_tmp1, path1) = temp_file(b"file one");
    let (_tmp2, path2) = temp_file(b"file two");
    let result = run_io(&format!(
        r#"
        local f1 = io.open("{path1}", "r")
        local f2 = io.open("{path2}", "r")
        local a = f1:read("*a")
        local b = f2:read("*a")
        f1:close()
        f2:close()
        return a, b
        "#
    ));
    k9::assert_equal!(
        result,
        vec![
            Value::String(Bytes::from("file one")),
            Value::String(Bytes::from("file two")),
        ]
    );
}

// ===========================================================================
// f:write with invalid argument type
// ===========================================================================

#[test]
fn write_invalid_arg_type() {
    let (_dir, path) = temp_dir_file("bad_write.txt");
    let err = run_io_err(&format!(
        r#"
        local f = io.open("{path}", "w")
        f:write(true)
        "#
    ));
    assert!(err.contains("write"), "got: {err}");
}

// ===========================================================================
// io.open with "w+" and "a+" through Lua
// ===========================================================================

#[test]
fn io_open_write_plus_through_lua() {
    let (_dir, path) = temp_dir_file("wplus.txt");
    let result = run_io_one(&format!(
        r#"
        local f = io.open("{path}", "w+")
        f:write("hello")
        f:seek("set", 0)
        local data = f:read("*a")
        f:close()
        return data
        "#
    ));
    k9::assert_equal!(result, Value::String(Bytes::from("hello")));
}

#[test]
fn io_open_append_plus_through_lua() {
    let (_tmp, path) = temp_file(b"old ");
    let result = run_io_one(&format!(
        r#"
        local f = io.open("{path}", "a+")
        f:write("new")
        f:seek("set", 0)
        local data = f:read("*a")
        f:close()
        return data
        "#
    ));
    k9::assert_equal!(result, Value::String(Bytes::from("old new")));
}
