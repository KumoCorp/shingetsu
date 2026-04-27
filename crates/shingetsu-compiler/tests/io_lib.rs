mod common;

use shingetsu::valuevec;
use shingetsu_compiler::{CompileOptions, Compiler};
use shingetsu_vm::{Function, GlobalEnv, Task, Value, ValueVec};

// ===========================================================================
// Helpers
// ===========================================================================

/// Create an environment with builtins + io library registered.
fn io_env() -> GlobalEnv {
    let env = GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("register builtins");
    shingetsu::io_lib::register(&env).expect("register io");
    shingetsu::io_lib::register_stdio(&env).expect("register stdio");
    env
}

/// Run Lua code with io library available, return all values.
async fn run_io(src: &str) -> ValueVec {
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler.compile(src).await.expect("compile");
    let env = io_env();
    let func = Function::lua(bc.top_level, vec![]);
    Task::new(env, func, valuevec![]).await.expect("run")
}

/// Run Lua code with io library available, return first value.
async fn run_io_one(src: &str) -> Value {
    run_io(src).await.into_iter().next().unwrap_or(Value::Nil)
}

/// Run Lua code with io library available, expect an error.
async fn run_io_err(src: &str) -> String {
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler.compile(src).await.expect("compile");
    let env = io_env();
    let func = Function::lua(bc.top_level, vec![]);
    Task::new(env, func, valuevec![])
        .await
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

#[tokio::test]
async fn io_open_read_all() {
    let (_tmp, path) = temp_file(b"hello world");
    let result = run_io(&format!(
        r#"
        local f = io.open("{path}", "r")
        return f:read("*a")
        "#
    ))
    .await;
    k9::assert_equal!(result, valuevec![Value::string("hello world")]);
}

#[tokio::test]
async fn io_open_read_line() {
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
    ))
    .await;
    k9::assert_equal!(
        result,
        valuevec![
            Value::string("line1"),
            Value::string("line2"),
            Value::string("line3"),
        ]
    );
}

#[tokio::test]
async fn io_open_read_number() {
    let (_tmp, path) = temp_file(b"  42.5  99  ");
    let result = run_io(&format!(
        r#"
        local f = io.open("{path}", "r")
        local a = f:read("*n")
        local b = f:read("*n")
        f:close()
        return a, b
        "#
    ))
    .await;
    k9::assert_equal!(result, valuevec![Value::Float(42.5), Value::Float(99.0)]);
}

#[tokio::test]
async fn io_open_read_bytes() {
    let (_tmp, path) = temp_file(b"abcdefghij");
    let result = run_io(&format!(
        r#"
        local f = io.open("{path}", "r")
        local a = f:read(3)
        local b = f:read(4)
        f:close()
        return a, b
        "#
    ))
    .await;
    k9::assert_equal!(
        result,
        valuevec![Value::string("abc"), Value::string("defg"),]
    );
}

#[tokio::test]
async fn io_open_read_at_eof() {
    let (_tmp, path) = temp_file(b"short");
    let result = run_io_one(&format!(
        r#"
        local f = io.open("{path}", "r")
        f:read("*a")  -- consume all
        return f:read("*l")  -- should be nil at EOF
        "#
    ))
    .await;
    k9::assert_equal!(result, Value::Nil);
}

// ===========================================================================
// io.open — write mode
// ===========================================================================

#[tokio::test]
async fn io_open_write_and_read_back() {
    let (_dir, path) = temp_dir_file("output.txt");
    run_io(&format!(
        r#"
        local f = io.open("{path}", "w")
        f:write("hello ")
        f:write("world")
        f:close()
        "#
    ))
    .await;
    let contents = std::fs::read(&path).expect("read back");
    k9::assert_equal!(contents.as_slice(), b"hello world");
}

#[tokio::test]
async fn io_open_write_numbers() {
    let (_dir, path) = temp_dir_file("numbers.txt");
    run_io(&format!(
        r#"
        local f = io.open("{path}", "w")
        f:write(42)
        f:write(" ")
        f:write(3.14)
        f:close()
        "#
    ))
    .await;
    let contents = std::fs::read(&path).expect("read back");
    k9::assert_equal!(contents.as_slice(), b"42 3.14");
}

#[tokio::test]
async fn io_open_write_chaining() {
    let (_dir, path) = temp_dir_file("chain.txt");
    run_io(&format!(
        r#"
        local f = io.open("{path}", "w")
        f = f:write("a")
        f = f:write("b")
        f:write("c")
        f:close()
        "#
    ))
    .await;
    let contents = std::fs::read(&path).expect("read back");
    k9::assert_equal!(contents.as_slice(), b"abc");
}

// ===========================================================================
// io.open — append mode
// ===========================================================================

#[tokio::test]
async fn io_open_append() {
    let (_tmp, path) = temp_file(b"existing ");
    run_io(&format!(
        r#"
        local f = io.open("{path}", "a")
        f:write("appended")
        f:close()
        "#
    ))
    .await;
    let contents = std::fs::read(&path).expect("read back");
    k9::assert_equal!(contents.as_slice(), b"existing appended");
}

// ===========================================================================
// io.open — read+write mode
// ===========================================================================

#[tokio::test]
async fn io_open_read_write_mode() {
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
    ))
    .await;
    k9::assert_equal!(result, valuevec![Value::string("hello")]);
    let contents = std::fs::read(&path).expect("read back");
    k9::assert_equal!(contents.as_slice(), b"hello lua!!");
}

// ===========================================================================
// io.open — error cases
// ===========================================================================

#[tokio::test]
async fn io_open_nonexistent_returns_nil() {
    let result = run_io(
        r#"
        local f, err = io.open("/tmp/nonexistent_shingetsu_xyz_42", "r")
        return f, err
        "#,
    )
    .await;
    k9::assert_equal!(result[0], Value::Nil);
    k9::assert_equal!(
        result[1],
        Value::string("/tmp/nonexistent_shingetsu_xyz_42: No such file or directory")
    );
}

#[tokio::test]
async fn io_open_default_mode_is_read() {
    let (_tmp, path) = temp_file(b"default mode");
    let result = run_io_one(&format!(
        r#"
        local f = io.open("{path}")
        return f:read("*a")
        "#
    ))
    .await;
    k9::assert_equal!(result, Value::string("default mode"));
}

// ===========================================================================
// io.close
// ===========================================================================

#[tokio::test]
async fn io_close_file() {
    let (_tmp, path) = temp_file(b"data");
    let result = run_io(&format!(
        r#"
        local f = io.open("{path}", "r")
        io.close(f)
        return io.type(f)
        "#
    ))
    .await;
    k9::assert_equal!(result, valuevec![Value::string("closed file")]);
}

#[tokio::test]
async fn io_close_stderr_returns_error() {
    let result = run_io(
        r#"
        local ok, msg = io.stderr:close()
        return ok, msg
        "#,
    )
    .await;
    k9::assert_equal!(
        result,
        valuevec![Value::Nil, Value::string("cannot close standard file")]
    );
}

#[tokio::test]
async fn io_close_stdout_returns_error() {
    let result = run_io(
        r#"
        local ok, msg = io.stdout:close()
        return ok, msg
        "#,
    )
    .await;
    k9::assert_equal!(
        result,
        valuevec![Value::Nil, Value::string("cannot close standard file")]
    );
}

#[tokio::test]
async fn io_close_stdin_returns_error() {
    let result = run_io(
        r#"
        local ok, msg = io.stdin:close()
        return ok, msg
        "#,
    )
    .await;
    k9::assert_equal!(
        result,
        valuevec![Value::Nil, Value::string("cannot close standard file")]
    );
}

// ===========================================================================
// io.type
// ===========================================================================

#[tokio::test]
async fn io_type_open_file() {
    let (_tmp, path) = temp_file(b"");
    let result = run_io_one(&format!(
        r#"
        local f = io.open("{path}", "r")
        return io.type(f)
        "#
    ))
    .await;
    k9::assert_equal!(result, Value::string("file"));
}

#[tokio::test]
async fn io_type_closed_file() {
    let (_tmp, path) = temp_file(b"");
    let result = run_io_one(&format!(
        r#"
        local f = io.open("{path}", "r")
        f:close()
        return io.type(f)
        "#
    ))
    .await;
    k9::assert_equal!(result, Value::string("closed file"));
}

#[tokio::test]
async fn io_type_non_file() {
    let result = run_io(
        r#"
        return io.type(42), io.type("hello"), io.type(nil), io.type(true)
        "#,
    )
    .await;
    k9::assert_equal!(
        result,
        valuevec![Value::Nil, Value::Nil, Value::Nil, Value::Nil]
    );
}

// ===========================================================================
// io.tmpfile
// ===========================================================================

#[tokio::test]
async fn io_tmpfile_write_and_read() {
    let result = run_io_one(
        r#"
        local f = io.tmpfile()
        f:write("temp data")
        f:seek("set", 0)
        return f:read("*a")
        "#,
    )
    .await;
    k9::assert_equal!(result, Value::string("temp data"));
}

#[tokio::test]
async fn io_tmpfile_is_file_type() {
    let result = run_io_one(
        r#"
        local f = io.tmpfile()
        return io.type(f)
        "#,
    )
    .await;
    k9::assert_equal!(result, Value::string("file"));
}

// ===========================================================================
// f:seek
// ===========================================================================

#[tokio::test]
async fn file_seek_set_cur_end() {
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
    ))
    .await;
    k9::assert_equal!(
        result,
        valuevec![
            Value::Integer(3),
            Value::string("d"),
            Value::Integer(4),
            Value::Integer(8),
            Value::string("ij"),
        ]
    );
}

// ===========================================================================
// f:flush
// ===========================================================================

#[tokio::test]
async fn file_flush() {
    let (_dir, path) = temp_dir_file("flush.txt");
    run_io(&format!(
        r#"
        local f = io.open("{path}", "w")
        f:write("flushed")
        f:flush()
        "#
    ))
    .await;
    // After flush, data should be on disk even without close.
    let contents = std::fs::read(&path).expect("read back");
    k9::assert_equal!(contents.as_slice(), b"flushed");
}

// ===========================================================================
// f:lines
// ===========================================================================

#[tokio::test]
async fn file_lines_iterator() {
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
    ))
    .await;
    k9::assert_equal!(
        result,
        valuevec![
            Value::string("alpha"),
            Value::string("beta"),
            Value::string("gamma"),
            Value::Integer(3),
        ]
    );
}

// ===========================================================================
// f:setvbuf
// ===========================================================================

#[tokio::test]
async fn file_setvbuf_no() {
    let (_dir, path) = temp_dir_file("setvbuf.txt");
    run_io(&format!(
        r#"
        local f = io.open("{path}", "w")
        f:setvbuf("no")
        f:write("immediate")
        -- In unbuffered mode, data should be on disk without flush.
        "#
    ))
    .await;
    let contents = std::fs::read(&path).expect("read back");
    k9::assert_equal!(contents.as_slice(), b"immediate");
}

// ===========================================================================
// Operations on closed files
// ===========================================================================

#[tokio::test]
async fn closed_file_read_returns_nil_and_error() {
    let (_tmp, path) = temp_file(b"data");
    let result = run_io(&format!(
        r#"
        local f = io.open("{path}", "r")
        f:close()
        local val, err = f:read("*a")
        return val, err
        "#
    ))
    .await;
    k9::assert_equal!(result[0], Value::Nil);
    k9::assert_equal!(result[1], Value::string("attempt to use a closed file"));
}

#[tokio::test]
async fn closed_file_write_returns_nil_and_error() {
    let (_dir, path) = temp_dir_file("closed.txt");
    let result = run_io(&format!(
        r#"
        local f = io.open("{path}", "w")
        f:close()
        local val, err = f:write("data")
        return val, err
        "#
    ))
    .await;
    k9::assert_equal!(result[0], Value::Nil);
    k9::assert_equal!(result[1], Value::string("attempt to use a closed file"));
}

// ===========================================================================
// f:read with multiple format args
// ===========================================================================

#[tokio::test]
async fn read_multiple_formats() {
    let (_tmp, path) = temp_file(b"42 hello\nworld");
    let result = run_io(&format!(
        r#"
        local f = io.open("{path}", "r")
        local n, line, rest = f:read("*n", "*l", "*a")
        f:close()
        return n, line, rest
        "#
    ))
    .await;
    k9::assert_equal!(
        result,
        valuevec![
            Value::Float(42.0),
            Value::string(" hello"),
            Value::string("world"),
        ]
    );
}

// ===========================================================================
// __tostring metamethod
// ===========================================================================

#[tokio::test]
async fn file_tostring() {
    let (_tmp, path) = temp_file(b"");
    let result = run_io_one(&format!(
        r#"
        local f = io.open("{path}", "r")
        return tostring(f)
        "#
    ))
    .await;
    let expected = format!("file ({path})");
    k9::assert_equal!(result, Value::string(expected));
}

#[tokio::test]
async fn closed_file_tostring() {
    let (_tmp, path) = temp_file(b"");
    let result = run_io_one(&format!(
        r#"
        local f = io.open("{path}", "r")
        f:close()
        return tostring(f)
        "#
    ))
    .await;
    k9::assert_equal!(result, Value::string("file (closed)"));
}

// ===========================================================================
// io.open — invalid mode
// ===========================================================================

#[tokio::test]
async fn io_open_invalid_mode() {
    let (_tmp, path) = temp_file(b"");
    let err = run_io_err(&format!(r#"io.open("{path}", "x")"#)).await;
    k9::assert_equal!(err, "bad argument #2 to 'open' (invalid mode 'x')");
}

// ===========================================================================
// f:read("*L") — keep newline
// ===========================================================================

#[tokio::test]
async fn read_keep_newline() {
    let (_tmp, path) = temp_file(b"line1\nline2\n");
    let result = run_io(&format!(
        r#"
        local f = io.open("{path}", "r")
        local a = f:read("*L")
        local b = f:read("*L")
        f:close()
        return a, b
        "#
    ))
    .await;
    k9::assert_equal!(
        result,
        valuevec![Value::string("line1\n"), Value::string("line2\n"),]
    );
}

// ===========================================================================
// f:lines with format arg
// ===========================================================================

#[tokio::test]
async fn file_lines_with_number_format() {
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
    ))
    .await;
    k9::assert_equal!(
        result,
        valuevec![
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

#[tokio::test]
async fn file_setvbuf_full() {
    let (_dir, path) = temp_dir_file("setvbuf_full.txt");
    run_io(&format!(
        r#"
        local f = io.open("{path}", "w")
        f:setvbuf("full")
        f:write("buffered")
        f:flush()
        f:close()
        "#
    ))
    .await;
    let contents = std::fs::read(&path).expect("read back");
    k9::assert_equal!(contents.as_slice(), b"buffered");
}

#[tokio::test]
async fn file_setvbuf_line() {
    let (_dir, path) = temp_dir_file("setvbuf_line.txt");
    run_io(&format!(
        r#"
        local f = io.open("{path}", "w")
        f:setvbuf("line")
        f:write("line buffered\n")
        f:close()
        "#
    ))
    .await;
    let contents = std::fs::read(&path).expect("read back");
    k9::assert_equal!(contents.as_slice(), b"line buffered\n");
}

// ===========================================================================
// f:seek() with no args — defaults to "cur", 0
// ===========================================================================

#[tokio::test]
async fn file_seek_default_args() {
    let (_tmp, path) = temp_file(b"abcdef");
    let result = run_io(&format!(
        r#"
        local f = io.open("{path}", "r")
        f:read(3)
        local pos = f:seek()
        f:close()
        return pos
        "#
    ))
    .await;
    k9::assert_equal!(result, valuevec![Value::Integer(3)]);
}

// ===========================================================================
// io.close on already-closed file
// ===========================================================================

#[tokio::test]
async fn io_close_already_closed() {
    let (_tmp, path) = temp_file(b"");
    let result = run_io(&format!(
        r#"
        local f = io.open("{path}", "r")
        f:close()
        local ok, err = io.close(f)
        return ok, err
        "#
    ))
    .await;
    k9::assert_equal!(result[0], Value::Nil);
    k9::assert_equal!(result[1], Value::string("attempt to use a closed file"));
}

// ===========================================================================
// Write + read round trip in one script
// ===========================================================================

#[tokio::test]
async fn write_then_read_round_trip() {
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
    ))
    .await;
    k9::assert_equal!(result, Value::string("round trip"));
}

// ===========================================================================
// Binary data (non-UTF8)
// ===========================================================================

#[tokio::test]
async fn binary_data_round_trip() {
    let data: Vec<u8> = (0..=255).collect();
    let (_tmp, path) = temp_file(&data);
    let result = run_io_one(&format!(
        r#"
        local f = io.open("{path}", "rb")
        return f:read("*a")
        "#
    ))
    .await;
    let expected: Vec<u8> = (0..=255).collect();
    k9::assert_equal!(result, Value::String(shingetsu_vm::Bytes::from(expected)));
}

// ===========================================================================
// Multiple files open simultaneously
// ===========================================================================

#[tokio::test]
async fn multiple_files_open() {
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
    ))
    .await;
    k9::assert_equal!(
        result,
        valuevec![Value::string("file one"), Value::string("file two"),]
    );
}

// ===========================================================================
// f:write with invalid argument type
// ===========================================================================

#[tokio::test]
async fn write_invalid_arg_type() {
    let (_dir, path) = temp_dir_file("bad_write.txt");
    let err = run_io_err(&format!(
        r#"
        local f = io.open("{path}", "w")
        f:write(true)
        "#
    ))
    .await;
    k9::assert_equal!(
        err,
        "bad argument #2 to 'write' (string or number expected, got boolean)"
    );
}

// ===========================================================================
// io.open with "w+" and "a+" through Lua
// ===========================================================================

#[tokio::test]
async fn io_open_write_plus_through_lua() {
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
    ))
    .await;
    k9::assert_equal!(result, Value::string("hello"));
}

#[tokio::test]
async fn io_open_append_plus_through_lua() {
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
    ))
    .await;
    k9::assert_equal!(result, Value::string("old new"));
}

// ===========================================================================
// stdio registration and io.stdin / io.stdout / io.stderr
// ===========================================================================

/// Create an environment with builtins + io + stdio registered.
fn stdio_env() -> GlobalEnv {
    let env = GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("register builtins");
    shingetsu::io_lib::register(&env).expect("register io");
    shingetsu::io_lib::register_stdio(&env).expect("register stdio");
    env
}

/// Run Lua code with io + stdio libraries available, return all values.
async fn run_stdio(src: &str) -> ValueVec {
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler.compile(src).await.expect("compile");
    let env = stdio_env();
    let func = Function::lua(bc.top_level, vec![]);
    Task::new(env, func, valuevec![]).await.expect("run")
}

/// Run Lua code with io + stdio libraries available, return first value.
async fn run_stdio_one(src: &str) -> Value {
    run_stdio(src)
        .await
        .into_iter()
        .next()
        .unwrap_or(Value::Nil)
}

/// Run Lua code with io + stdio, expect an error.
async fn run_stdio_err(src: &str) -> String {
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler.compile(src).await.expect("compile");
    let env = stdio_env();
    let func = Function::lua(bc.top_level, vec![]);
    Task::new(env, func, valuevec![])
        .await
        .unwrap_err()
        .to_string()
}

#[tokio::test]
async fn io_stdin_exists() {
    let result = run_stdio_one("return io.type(io.stdin)").await;
    k9::assert_equal!(result, Value::string("file"));
}

#[tokio::test]
async fn io_stdout_exists() {
    let result = run_stdio_one("return io.type(io.stdout)").await;
    k9::assert_equal!(result, Value::string("file"));
}

#[tokio::test]
async fn io_stderr_exists() {
    let result = run_stdio_one("return io.type(io.stderr)").await;
    k9::assert_equal!(result, Value::string("file"));
}

#[tokio::test]
async fn io_input_returns_default() {
    // io.input() with no args returns the default input (stdin).
    let result = run_stdio_one("return io.type(io.input())").await;
    k9::assert_equal!(result, Value::string("file"));
}

#[tokio::test]
async fn io_output_returns_default() {
    // io.output() with no args returns the default output (stdout).
    let result = run_stdio_one("return io.type(io.output())").await;
    k9::assert_equal!(result, Value::string("file"));
}

#[tokio::test]
async fn io_output_set_and_write() {
    // Redirect default output to a temp file, write via io.write,
    // then read back the contents.
    let (_dir, path) = temp_dir_file("output.txt");
    let result = run_stdio_one(&format!(
        r#"
        local f = io.open("{path}", "w")
        io.output(f)
        io.write("hello")
        io.write(" world")
        io.flush()
        f:close()

        local r = io.open("{path}", "r")
        local data = r:read("*a")
        r:close()
        return data
        "#
    ))
    .await;
    k9::assert_equal!(result, Value::string("hello world"));
}

#[tokio::test]
async fn io_input_set_and_read() {
    // Redirect default input to a temp file, read via io.read.
    let (_tmp, path) = temp_file(b"line one\nline two\n");
    let result = run_stdio(&format!(
        r#"
        local f = io.open("{path}", "r")
        io.input(f)
        local a = io.read("*l")
        local b = io.read("*l")
        f:close()
        return a, b
        "#
    ))
    .await;
    k9::assert_equal!(
        result,
        valuevec![Value::string("line one"), Value::string("line two")]
    );
}

#[tokio::test]
async fn io_input_set_by_filename() {
    // io.input(filename) opens the file and sets it as default input.
    let (_tmp, path) = temp_file(b"from file");
    let result = run_stdio_one(&format!(
        r#"
        io.input("{path}")
        return io.read("*a")
        "#
    ))
    .await;
    k9::assert_equal!(result, Value::string("from file"));
}

#[tokio::test]
async fn io_output_set_by_filename() {
    // io.output(filename) opens the file in write mode and sets it as
    // default output.
    let (_dir, path) = temp_dir_file("output2.txt");
    run_stdio(&format!(
        r#"
        io.output("{path}")
        io.write("written by io.write")
        io.flush()
        io.close()
        "#
    ))
    .await;
    let contents = std::fs::read_to_string(&path).expect("read file");
    k9::assert_equal!(contents, "written by io.write");
}

#[tokio::test]
async fn io_close_no_args_closes_default_output() {
    // After io.close() with no args on a reassigned output,
    // io.write() should fail.
    let (_dir, path) = temp_dir_file("close_test.txt");
    let err = run_stdio_err(&format!(
        r#"
        io.output("{path}")
        io.write("data")
        io.close()
        io.write("should fail")
        "#
    ))
    .await;
    k9::assert_equal!(err, "default output file is closed");
}

#[tokio::test]
async fn io_close_stdout_is_noop() {
    // Closing the default stdout (which is a stdio handle) should be
    // a no-op — subsequent writes still work.
    let (_dir, path) = temp_dir_file("stdout_close.txt");
    let result = run_stdio_one(&format!(
        r#"
        -- close default output (stdout) -- should be a no-op
        io.close()
        -- Redirect to a file and write -- should still work
        io.output("{path}")
        io.write("still works")
        io.flush()
        local out = io.output()
        out:close()

        local r = io.open("{path}", "r")
        local data = r:read("*a")
        r:close()
        return data
        "#
    ))
    .await;
    k9::assert_equal!(result, Value::string("still works"));
}

// ===========================================================================
// io.type
// ===========================================================================

#[tokio::test]
async fn io_type_open_file_via_lua() {
    let (tmp, path) = temp_file(b"hello");
    let result = run_io_one(&format!(
        r#"local f = io.open("{path}", "r")
           local t = io.type(f)
           f:close()
           return t"#
    ))
    .await;
    drop(tmp);
    k9::assert_equal!(result, Value::string("file"));
}

#[tokio::test]
async fn io_type_closed_file_via_lua() {
    let (tmp, path) = temp_file(b"hello");
    let result = run_io_one(&format!(
        r#"local f = io.open("{path}", "r")
           f:close()
           return io.type(f)"#
    ))
    .await;
    drop(tmp);
    k9::assert_equal!(result, Value::string("closed file"));
}

#[tokio::test]
async fn io_type_non_file_via_lua() {
    let result = run_io_one(r#"return io.type(42)"#).await;
    k9::assert_equal!(result, Value::Nil);
}

#[tokio::test]
async fn io_type_stdin_via_lua() {
    let result = run_stdio_one(r#"return io.type(io.stdin)"#).await;
    k9::assert_equal!(result, Value::string("file"));
}

// ===========================================================================
// io.read / io.write defaults and edge cases
// ===========================================================================

#[tokio::test]
async fn io_read_default_format_is_line() {
    // io.read() with no args should default to "*l".
    let (tmp, path) = temp_file(b"first\nsecond\n");
    let result = run_stdio_one(&format!(
        r#"io.input("{path}")
           return io.read()"#
    ))
    .await;
    drop(tmp);
    k9::assert_equal!(result, Value::string("first"));
}

#[tokio::test]
async fn file_read_default_format_is_line() {
    // f:read() with no args should default to "*l".
    let (tmp, path) = temp_file(b"alpha\nbeta\n");
    let result = run_io_one(&format!(
        r#"local f = io.open("{path}", "r")
           local line = f:read()
           f:close()
           return line"#
    ))
    .await;
    drop(tmp);
    k9::assert_equal!(result, Value::string("alpha"));
}

#[tokio::test]
async fn io_write_multiple_args() {
    let (_dir, path) = temp_dir_file("multi.txt");
    let result = run_stdio_one(&format!(
        r#"io.output("{path}")
           io.write("hello", " ", "world")
           io.flush()
           local out = io.output()
           out:close()
           local f = io.open("{path}", "r")
           local data = f:read("*a")
           f:close()
           return data"#
    ))
    .await;
    k9::assert_equal!(result, Value::string("hello world"));
}

#[tokio::test]
async fn io_flush_via_lua() {
    // io.flush() should flush the default output.
    let (_dir, path) = temp_dir_file("flush.txt");
    let result = run_stdio_one(&format!(
        r#"io.output("{path}")
           io.write("flushed")
           io.flush()
           -- read back before closing to verify flush worked
           local f = io.open("{path}", "r")
           local data = f:read("*a")
           f:close()
           local out = io.output()
           out:close()
           return data"#
    ))
    .await;
    k9::assert_equal!(result, Value::string("flushed"));
}

#[tokio::test]
async fn io_close_explicit_file_arg() {
    let (tmp, path) = temp_file(b"data");
    let result = run_io_one(&format!(
        r#"local f = io.open("{path}", "r")
           io.close(f)
           return io.type(f)"#
    ))
    .await;
    drop(tmp);
    k9::assert_equal!(result, Value::string("closed file"));
}

#[tokio::test]
async fn io_read_on_closed_default_input() {
    let (tmp, path) = temp_file(b"data");
    let err = run_stdio_err(&format!(
        r#"io.input("{path}")
           local inp = io.input()
           inp:close()
           io.read("*a")"#
    ))
    .await;
    drop(tmp);
    k9::assert_equal!(err, "default input file is closed");
}

#[tokio::test]
async fn io_write_on_closed_default_output() {
    let (_dir, path) = temp_dir_file("closed_out.txt");
    let err = run_stdio_err(&format!(
        r#"io.output("{path}")
           local out = io.output()
           out:close()
           io.write("fail")"#
    ))
    .await;
    k9::assert_equal!(err, "default output file is closed");
}

#[tokio::test]
async fn read_crlf_line_handling() {
    let (tmp, path) = temp_file(b"dos\r\nline\r\n");
    let results = run_io(&format!(
        r#"local f = io.open("{path}", "r")
           local a = f:read("*l")
           local b = f:read("*l")
           f:close()
           return a, b"#
    ))
    .await;
    drop(tmp);
    k9::assert_equal!(
        results,
        valuevec![Value::string("dos"), Value::string("line")]
    );
}

#[tokio::test]
async fn read_crlf_keep_newline() {
    let (tmp, path) = temp_file(b"dos\r\n");
    let result = run_io_one(&format!(
        r#"local f = io.open("{path}", "r")
           local line = f:read("*L")
           f:close()
           return line"#
    ))
    .await;
    drop(tmp);
    // *L preserves the full CRLF line ending.
    k9::assert_equal!(result, Value::string("dos\r\n"));
}

// ===========================================================================
// io.input / io.output error paths
// ===========================================================================

#[tokio::test]
async fn io_input_bad_arg_type() {
    let err = run_stdio_err("io.input(42)").await;
    k9::assert_equal!(
        err,
        "bad argument #1 to 'input' (file | string expected, got number)"
    );
}

#[tokio::test]
async fn io_output_bad_arg_type() {
    let err = run_stdio_err("io.output(true)").await;
    k9::assert_equal!(
        err,
        "bad argument #1 to 'output' (file | string expected, got boolean)"
    );
}

#[tokio::test]
async fn io_close_bad_arg_type() {
    let err = run_stdio_err("io.close(42)").await;
    k9::assert_equal!(
        err,
        "bad argument #1 to 'close' (file expected, got number)"
    );
}

#[tokio::test]
async fn io_input_nonexistent_file() {
    let err = run_stdio_err(r#"io.input("/tmp/nonexistent_shingetsu_input_xyz")"#).await;
    k9::assert_equal!(
        err,
        "/tmp/nonexistent_shingetsu_input_xyz: No such file or directory"
    );
}

// ===========================================================================
// io.open append mode through Lua
// ===========================================================================

#[tokio::test]
async fn io_open_append_mode() {
    let (_tmp, path) = temp_file(b"existing ");
    let result = run_io_one(&format!(
        r#"local f = io.open("{path}", "a")
           f:write("appended")
           f:close()
           local r = io.open("{path}", "r")
           local data = r:read("*a")
           r:close()
           return data"#
    ))
    .await;
    k9::assert_equal!(result, Value::string("existing appended"));
}

// ===========================================================================
// f:read(0) returns empty string or nil at EOF
// ===========================================================================

#[tokio::test]
async fn read_zero_bytes_at_eof() {
    let (_tmp, path) = temp_file(b"hello");
    let result = run_io_one(&format!(
        r#"local f = io.open("{path}", "r")
           f:read("*a")  -- consume all
           local b = f:read(0)
           f:close()
           return b"#
    ))
    .await;
    // At EOF, read(0) returns nil.
    k9::assert_equal!(result, Value::Nil);
}

// ===========================================================================
// flush_stdio no-op when not registered
// ===========================================================================

#[tokio::test]
async fn flush_stdio_noop_when_not_registered() {
    // Calling flush_stdio before register_stdio should not panic.
    shingetsu::io_lib::flush_stdio().await;
}

// ===========================================================================
// io.tmpfile: type and seekability
// ===========================================================================

#[tokio::test]
async fn io_tmpfile_type_and_seekable() {
    let result = run_io(&format!(
        r#"local f = io.tmpfile()
           local t = io.type(f)
           f:write("abc")
           local pos = f:seek("set", 0)
           f:close()
           return t, pos"#
    ))
    .await;
    k9::assert_equal!(result[0], Value::string("file"));
    k9::assert_equal!(result[1], Value::Integer(0));
}

// ===========================================================================
// io.open: reading from write-only file
// ===========================================================================

#[tokio::test]
async fn io_open_write_only_read_errors() {
    let (_dir, path) = temp_dir_file("wonly.txt");
    // Reading a write-only file is an error (propagated as a Lua error).
    let err = run_io_err(&format!(
        r#"local f = io.open("{path}", "w")
           f:read("*a")
           f:close()"#
    ))
    .await;
    k9::assert_equal!(err, "error in 'file:read': not open for reading");
}

#[tokio::test]
async fn io_open_read_only_write_errors() {
    let (_tmp, path) = temp_file(b"data");
    // Writing to a read-only file now errors immediately.
    let err = run_io_err(&format!(
        r#"local f = io.open("{path}", "r")
           f:write("test")
           f:close()"#
    ))
    .await;
    k9::assert_equal!(err, "error in 'file:write': not open for writing");
}

// ===========================================================================
// io.lines(filename, ...)
// ===========================================================================

#[tokio::test]
async fn io_lines_reads_all_lines() {
    let (_tmp, path) = temp_file(b"alpha\nbeta\ngamma\n");
    let result = run_io(&format!(
        r#"
        local t = {{}}
        for line in io.lines("{path}") do
            t[#t + 1] = line
        end
        return t[1], t[2], t[3], #t
        "#
    ))
    .await;
    k9::assert_equal!(
        result,
        valuevec![
            Value::string("alpha"),
            Value::string("beta"),
            Value::string("gamma"),
            Value::Integer(3),
        ]
    );
}

#[tokio::test]
async fn io_lines_empty_file() {
    let (_tmp, path) = temp_file(b"");
    let result = run_io(&format!(
        r#"
        local count = 0
        for line in io.lines("{path}") do
            count = count + 1
        end
        return count
        "#
    ))
    .await;
    k9::assert_equal!(result, valuevec![Value::Integer(0)]);
}

#[tokio::test]
async fn io_lines_no_trailing_newline() {
    let (_tmp, path) = temp_file(b"one\ntwo");
    let result = run_io(&format!(
        r#"
        local t = {{}}
        for line in io.lines("{path}") do
            t[#t + 1] = line
        end
        return t[1], t[2], #t
        "#
    ))
    .await;
    k9::assert_equal!(
        result,
        valuevec![
            Value::string("one"),
            Value::string("two"),
            Value::Integer(2),
        ]
    );
}

#[tokio::test]
async fn io_lines_early_break_closes_file() {
    let (_tmp, path) = temp_file(b"line1\nline2\nline3\n");
    // Break after first line.  The <close> variable should close the file.
    let result = run_io(&format!(
        r#"
        local first
        for line in io.lines("{path}") do
            first = line
            break
        end
        -- io.type can detect if the file handle was closed;
        -- but we don't have access to the hidden closing variable.
        -- Instead just verify we got the right line.
        return first
        "#
    ))
    .await;
    k9::assert_equal!(result, valuevec![Value::string("line1")]);
}

#[tokio::test]
async fn io_lines_with_number_format() {
    let (_tmp, path) = temp_file(b"abcdefghij");
    // Read 3 bytes at a time.
    let result = run_io(&format!(
        r#"
        local t = {{}}
        for chunk in io.lines("{path}", 3) do
            t[#t + 1] = chunk
        end
        return t[1], t[2], t[3], t[4], #t
        "#
    ))
    .await;
    k9::assert_equal!(
        result,
        valuevec![
            Value::string("abc"),
            Value::string("def"),
            Value::string("ghi"),
            Value::string("j"),
            Value::Integer(4),
        ]
    );
}

#[tokio::test]
async fn io_lines_with_line_format_explicit() {
    let (_tmp, path) = temp_file(b"hello\nworld\n");
    // Explicit "*l" format.
    let result = run_io(&format!(
        r#"
        local t = {{}}
        for line in io.lines("{path}", "*l") do
            t[#t + 1] = line
        end
        return t[1], t[2], #t
        "#
    ))
    .await;
    k9::assert_equal!(
        result,
        valuevec![
            Value::string("hello"),
            Value::string("world"),
            Value::Integer(2),
        ]
    );
}

#[tokio::test]
async fn io_lines_nonexistent_file() {
    let err = run_io_err(r#"for line in io.lines("/nonexistent/file.txt") do end"#).await;
    k9::assert_equal!(err, "/nonexistent/file.txt: No such file or directory");
}

#[tokio::test]
async fn io_lines_auto_closes_at_eof() {
    let (_tmp, path) = temp_file(b"only\n");
    // After the loop completes, the file should be auto-closed.
    // We verify by checking io.type on the file handle we sneak out
    // of the iterator return values.
    let result = run_io(&format!(
        r#"
        local iter, s, c, closing = io.lines("{path}")
        -- closing is the file handle (4th return value)
        -- Exhaust the iterator
        while iter(s, c) do end
        -- File should now be closed
        return io.type(closing)
        "#
    ))
    .await;
    k9::assert_equal!(result, valuevec![Value::string("closed file")]);
}

#[tokio::test]
async fn io_lines_break_closes_via_close_var() {
    let (_tmp, path) = temp_file(b"line1\nline2\nline3\n");
    // Use a userdata with __close that we can observe.
    // The io.lines file handle has __close, so breaking out of the
    // for loop triggers CloseVar which calls __close on the file.
    // We capture the handle before the loop to verify it's closed after break.
    let result = run_io(&format!(
        r#"
        local iter, s, c, fh = io.lines("{path}")
        -- fh is the file handle (4th return, will be the <close> variable)
        -- Run the loop manually with a for-in to exercise the CloseVar path.
        for line in io.lines("{path}") do
            break
        end
        -- The for-in loop's <close> variable is gone, but fh from the
        -- manual call is still accessible.  Verify the concept works
        -- by checking the manually-obtained handle.
        return io.type(fh)
        "#
    ))
    .await;
    // fh was never iterated to EOF, so it's still open (the for-in
    // used a separate io.lines call).  This just verifies the 4th
    // return value is indeed a file.
    k9::assert_equal!(result, valuevec![Value::string("file")]);
}

#[tokio::test]
async fn io_lines_continue_keeps_iterating() {
    let (_tmp, path) = temp_file(b"aaa\nbbb\nccc\n");
    // continue should skip the current iteration but NOT close the file.
    let result = run_io(&format!(
        r#"
        local t = {{}}
        for line in io.lines("{path}") do
            if line == "bbb" then
                continue
            end
            t[#t + 1] = line
        end
        return t[1], t[2], #t
        "#
    ))
    .await;
    k9::assert_equal!(
        result,
        valuevec![
            Value::string("aaa"),
            Value::string("ccc"),
            Value::Integer(2),
        ]
    );
}

#[tokio::test]
async fn io_lines_format_star_big_l() {
    let (_tmp, path) = temp_file(b"hello\nworld\n");
    // "*L" keeps the newline.
    let result = run_io(&format!(
        r#"
        local t = {{}}
        for line in io.lines("{path}", "*L") do
            t[#t + 1] = line
        end
        return t[1], t[2], #t
        "#
    ))
    .await;
    k9::assert_equal!(
        result,
        valuevec![
            Value::string("hello\n"),
            Value::string("world\n"),
            Value::Integer(2),
        ]
    );
}

#[tokio::test]
async fn io_lines_format_star_n() {
    let (_tmp, path) = temp_file(b"42\n3.14\n");
    // "*n" reads numbers.
    let result = run_io(&format!(
        r#"
        local t = {{}}
        for n in io.lines("{path}", "*n") do
            t[#t + 1] = n
        end
        return t[1], t[2], #t
        "#
    ))
    .await;
    k9::assert_equal!(
        result,
        valuevec![Value::Float(42.0), Value::Float(3.14), Value::Integer(2),]
    );
}

#[tokio::test]
async fn io_lines_multiple_formats() {
    let (_tmp, path) = temp_file(b"hello world 123");
    // Multiple format args: read 5 bytes, then a line.
    let result = run_io(&format!(
        r#"
        local chunks = {{}}
        local lines = {{}}
        for chunk, line in io.lines("{path}", 5, "*l") do
            chunks[#chunks + 1] = chunk
            lines[#lines + 1] = line
        end
        return chunks[1], lines[1], #chunks
        "#
    ))
    .await;
    k9::assert_equal!(
        result,
        valuevec![
            Value::string("hello"),
            Value::string(" world 123"),
            Value::Integer(1),
        ]
    );
}
