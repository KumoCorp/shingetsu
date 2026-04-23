use criterion::{criterion_group, criterion_main, Criterion};
use mlua::Lua as MLua;
use shingetsu::compiler::{CompileOptions, Compiler};
use shingetsu::{Function, GlobalEnv, Task};

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
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("tokio runtime");
    rt.block_on(async {
        let compiler = Compiler::new(CompileOptions::default(), Default::default());
        let bc = compiler.compile(src).await.expect("compile");
        let env = GlobalEnv::new();
        shingetsu::builtins::register(&env).expect("register builtins");
        let func = Function::lua(bc.top_level, vec![]);
        Task::new(env, func, vec![]).await.expect("run");
    });
}

fn run_mlua(src: &str) {
    let lua = MLua::new();
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

criterion_group!(benches, bench_int, bench_fib, bench_string, bench_table);
criterion_main!(benches);
