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

const BENCH_TABLE: &str = r#"
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

fn bench_table(c: &mut Criterion) {
    let mut group = c.benchmark_group("table_ops");
    group.bench_function("shingetsu", |b| b.iter(|| run_shingetsu(BENCH_TABLE)));
    group.bench_function("lua54", |b| b.iter(|| run_mlua(BENCH_TABLE)));
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
    bench_table,
    bench_native_dispatch
);
criterion_main!(benches);
