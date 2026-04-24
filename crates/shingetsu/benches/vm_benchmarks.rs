use criterion::{criterion_group, criterion_main, Criterion};
use mlua::Lua as MLua;
use shingetsu::compiler::{CompileOptions, Compiler};
use shingetsu::{Function, GlobalEnv, Task, VmError};

const BENCH_INT: &str = r#"
local sum = 0
for i = 1, 5000000 do
    sum = sum + i
end
return sum
"#;

const BENCH_FIB: &str = r#"
local function fib(n)
    if n < 2 then return n end
    return fib(n - 1) + fib(n - 2)
end
return fib(32)
"#;

const BENCH_STRING: &str = r#"
local s = ""
local total = 0
for i = 1, 500000 do
    s = s .. "x"
    total = total + #s
end
return total
"#;

const BENCH_NATIVE_DISPATCH: &str = r#"
local classify = classify
local transform_a = transform_a
local transform_b = transform_b
local check_threshold = check_threshold
local total = 0
for i = 1, 500000 do
    local kind = classify(i)
    if kind == 1 then
        total = total + transform_a(i)
    elseif kind == 2 then
        total = total + transform_b(i, i + 1)
    elseif kind == 3 then
        if check_threshold(i) then
            total = total + transform_a(i)
        else
            total = total + transform_b(i, 0)
        end
    end
end
return total
"#;

const BENCH_TABLE_INT: &str = r#"
local t = {}
local sum = 0
for i = 1, 1000000 do
    t[i] = i
end
for i = 1, 1000000 do
    sum = sum + t[i]
end
return sum
"#;

const BENCH_TABLE_STRING: &str = r#"
local keys = {
    "alpha", "bravo", "charlie", "delta", "echo",
    "foxtrot", "golf", "hotel", "india", "juliet"
}
local t = {}
local sum = 0
for i = 1, 500000 do
    local k = keys[(i % 10) + 1]
    t[k] = i
end
for i = 1, 500000 do
    local k = keys[(i % 10) + 1]
    sum = sum + t[k]
end
return sum
"#;

const BENCH_TABLE_MIXED: &str = r#"
local skeys = {}
for i = 0, 99 do
    skeys[i] = "k" .. tostring(i)
end
local t = {}
local sum = 0
for i = 1, 500000 do
    t[i] = i
    t[skeys[i % 100]] = i
end
for i = 1, 500000 do
    sum = sum + t[i]
    sum = sum + t[skeys[i % 100]]
end
return sum
"#;

const BENCH_TABLE_SMALL: &str = r#"
local sum = 0
for i = 1, 500000 do
    local t = { x = i, y = i + 1, z = i + 2 }
    sum = sum + t.x + t.y + t.z
end
return sum
"#;

fn run_shingetsu(src: &str) {
    run_shingetsu_with(src, |_| {});
}

fn run_shingetsu_with(src: &str, setup: impl FnOnce(&GlobalEnv)) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("tokio runtime");
    rt.block_on(async {
        let compiler = Compiler::new(CompileOptions::default(), Default::default());
        let bc = compiler.compile(src).await.expect("compile");
        let env = GlobalEnv::new();
        shingetsu::builtins::register(&env).expect("register builtins");
        setup(&env);
        let func = Function::lua(bc.top_level, vec![]);
        Task::new(env, func, vec![]).await.expect("run");
    });
}

fn run_mlua(src: &str) {
    run_mlua_with(src, |_| {});
}

fn run_mlua_with(src: &str, setup: impl FnOnce(&MLua)) {
    let lua = MLua::new();
    setup(&lua);
    lua.load(src).exec().expect("mlua exec");
}

fn bench_int(c: &mut Criterion) {
    let mut group = c.benchmark_group("int_loop");
    group.bench_function("shingetsu", |b| b.iter(|| run_shingetsu(BENCH_INT)));
    group.bench_function("lua54", |b| b.iter(|| run_mlua(BENCH_INT)));
    group.finish();
}

fn bench_fib(c: &mut Criterion) {
    let mut group = c.benchmark_group("fib");
    group.bench_function("shingetsu", |b| b.iter(|| run_shingetsu(BENCH_FIB)));
    group.bench_function("lua54", |b| b.iter(|| run_mlua(BENCH_FIB)));
    group.finish();
}

fn bench_string(c: &mut Criterion) {
    let mut group = c.benchmark_group("string_concat");
    group.bench_function("shingetsu", |b| b.iter(|| run_shingetsu(BENCH_STRING)));
    group.bench_function("lua54", |b| b.iter(|| run_mlua(BENCH_STRING)));
    group.finish();
}

fn bench_table_int(c: &mut Criterion) {
    let mut group = c.benchmark_group("table_int_keys");
    group.bench_function("shingetsu", |b| b.iter(|| run_shingetsu(BENCH_TABLE_INT)));
    group.bench_function("lua54", |b| b.iter(|| run_mlua(BENCH_TABLE_INT)));
    group.finish();
}

fn bench_table_string(c: &mut Criterion) {
    let mut group = c.benchmark_group("table_string_keys");
    group.bench_function("shingetsu", |b| {
        b.iter(|| run_shingetsu(BENCH_TABLE_STRING))
    });
    group.bench_function("lua54", |b| b.iter(|| run_mlua(BENCH_TABLE_STRING)));
    group.finish();
}

fn bench_table_mixed(c: &mut Criterion) {
    let mut group = c.benchmark_group("table_mixed_keys");
    group.bench_function("shingetsu", |b| b.iter(|| run_shingetsu(BENCH_TABLE_MIXED)));
    group.bench_function("lua54", |b| b.iter(|| run_mlua(BENCH_TABLE_MIXED)));
    group.finish();
}

fn bench_table_small(c: &mut Criterion) {
    let mut group = c.benchmark_group("table_small_construct");
    group.bench_function("shingetsu", |b| b.iter(|| run_shingetsu(BENCH_TABLE_SMALL)));
    group.bench_function("lua54", |b| b.iter(|| run_mlua(BENCH_TABLE_SMALL)));
    group.finish();
}

fn setup_natives_shingetsu(env: &GlobalEnv) {
    env.register_function(Function::wrap(
        "classify",
        |n: i64| -> Result<i64, VmError> { Ok((n % 3) + 1) },
    ));
    env.register_function(Function::wrap(
        "transform_a",
        |n: i64| -> Result<i64, VmError> { Ok(n.wrapping_mul(7).wrapping_add(3)) },
    ));
    env.register_function(Function::wrap(
        "transform_b",
        |a: i64, b: i64| -> Result<i64, VmError> { Ok(a.wrapping_add(b).wrapping_mul(13)) },
    ));
    env.register_function(Function::wrap(
        "check_threshold",
        |n: i64| -> Result<bool, VmError> { Ok(n % 5 < 2) },
    ));
}

fn setup_natives_mlua(lua: &MLua) {
    let globals = lua.globals();
    globals
        .set(
            "classify",
            lua.create_function(|_, n: i64| Ok((n % 3) + 1)).unwrap(),
        )
        .unwrap();
    globals
        .set(
            "transform_a",
            lua.create_function(|_, n: i64| Ok(n.wrapping_mul(7).wrapping_add(3)))
                .unwrap(),
        )
        .unwrap();
    globals
        .set(
            "transform_b",
            lua.create_function(|_, (a, b): (i64, i64)| Ok(a.wrapping_add(b).wrapping_mul(13)))
                .unwrap(),
        )
        .unwrap();
    globals
        .set(
            "check_threshold",
            lua.create_function(|_, n: i64| Ok(n % 5 < 2)).unwrap(),
        )
        .unwrap();
}

fn bench_native_dispatch(c: &mut Criterion) {
    let mut group = c.benchmark_group("native_dispatch");
    group.bench_function("shingetsu", |b| {
        b.iter(|| run_shingetsu_with(BENCH_NATIVE_DISPATCH, setup_natives_shingetsu))
    });
    group.bench_function("lua54", |b| {
        b.iter(|| run_mlua_with(BENCH_NATIVE_DISPATCH, setup_natives_mlua))
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_int,
    bench_fib,
    bench_string,
    bench_table_int,
    bench_table_string,
    bench_table_mixed,
    bench_table_small,
    bench_native_dispatch
);
criterion_main!(benches);
