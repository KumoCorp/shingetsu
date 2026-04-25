use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion};
use mlua::Lua as MLua;
use shingetsu::compiler::{CompileOptions, Compiler};
use shingetsu::{userdata, valuevec, Function, GlobalEnv, Task, Value, VmError};

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
        Task::new(env, func, valuevec![]).await.expect("run");
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

/// Reduce measurement time and sample count for benchmarks where a single
/// iteration takes hundreds of milliseconds or more. Without this, criterion's
/// default 100 samples × multi-second iterations can take several minutes per
/// benchmark group.
///
/// To get higher-confidence results for a specific benchmark, override from
/// the command line:
///   cargo bench -- --measurement-time=60 --sample-size=50 <filter>
fn cap_slow_benchmark(group: &mut criterion::BenchmarkGroup<criterion::measurement::WallTime>) {
    group.measurement_time(Duration::from_secs(8));
    group.sample_size(10);
}

fn bench_int(c: &mut Criterion) {
    let mut group = c.benchmark_group("int_loop");
    group.bench_function("shingetsu", |b| b.iter(|| run_shingetsu(BENCH_INT)));
    group.bench_function("lua54", |b| b.iter(|| run_mlua(BENCH_INT)));
    group.finish();
}

fn bench_fib(c: &mut Criterion) {
    let mut group = c.benchmark_group("fib");
    // Single iteration is ~1.2s; cap to avoid multi-minute runs.
    // Override with: cargo bench -- --measurement-time=60 fib
    cap_slow_benchmark(&mut group);
    group.bench_function("shingetsu", |b| b.iter(|| run_shingetsu(BENCH_FIB)));
    group.bench_function("lua54", |b| b.iter(|| run_mlua(BENCH_FIB)));
    group.finish();
}

fn bench_string(c: &mut Criterion) {
    let mut group = c.benchmark_group("string_concat");
    // Single iteration is ~3.7s; cap to avoid multi-minute runs.
    // Override with: cargo bench -- --measurement-time=60 string_concat
    cap_slow_benchmark(&mut group);
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

// ---------------------------------------------------------------------------
// Userdata method dispatch benchmark
// ---------------------------------------------------------------------------

const BENCH_USERDATA_METHODS: &str = r#"
local total = 0
for i = 1, 100000 do
    local subj = msg:get_header("subject")
    if subj then
        total = total + #subj
    end
    local p = msg:get_priority()
    total = total + p
    if i % 100 == 0 then
        msg:set_header("x-count", tostring(i))
    end
end
return total
"#;

struct Message {
    headers: RwLock<HashMap<String, String>>,
    priority: i64,
}

#[userdata]
impl Message {
    fn type_name(&self) -> &'static str {
        "Message"
    }

    #[lua_method]
    fn get_header(&self, name: String) -> Option<String> {
        self.headers.read().unwrap().get(&name).cloned()
    }

    #[lua_method]
    fn set_header(&self, name: String, value: String) {
        self.headers.write().unwrap().insert(name, value);
    }

    #[lua_method]
    fn get_priority(&self) -> i64 {
        self.priority
    }
}

fn setup_userdata_shingetsu(env: &GlobalEnv) {
    let mut headers = HashMap::new();
    headers.insert("subject".to_string(), "hello world".to_string());
    headers.insert("from".to_string(), "user@example.com".to_string());
    let msg = Arc::new(Message {
        headers: RwLock::new(headers),
        priority: 3,
    });
    env.set_global("msg", Value::Userdata(msg as Arc<dyn shingetsu::Userdata>));
}

fn setup_userdata_mlua(lua: &MLua) {
    let msg = lua.create_table().unwrap();
    let headers = Arc::new(RwLock::new({
        let mut h = HashMap::<String, String>::new();
        h.insert("subject".to_string(), "hello world".to_string());
        h.insert("from".to_string(), "user@example.com".to_string());
        h
    }));
    let priority: i64 = 3;
    {
        let headers = Arc::clone(&headers);
        msg.set(
            "get_header",
            lua.create_function(move |_, (_self, name): (mlua::Value, String)| {
                Ok(headers.read().unwrap().get(&name).cloned())
            })
            .unwrap(),
        )
        .unwrap();
    }
    {
        let headers = Arc::clone(&headers);
        msg.set(
            "set_header",
            lua.create_function(
                move |_, (_self, name, value): (mlua::Value, String, String)| {
                    headers.write().unwrap().insert(name, value);
                    Ok(())
                },
            )
            .unwrap(),
        )
        .unwrap();
    }
    msg.set(
        "get_priority",
        lua.create_function(move |_, _self: mlua::Value| Ok(priority))
            .unwrap(),
    )
    .unwrap();
    lua.globals().set("msg", msg).unwrap();
}

fn bench_userdata_methods(c: &mut Criterion) {
    let mut group = c.benchmark_group("userdata_methods");
    group.bench_function("shingetsu", |b| {
        b.iter(|| run_shingetsu_with(BENCH_USERDATA_METHODS, setup_userdata_shingetsu))
    });
    group.bench_function("lua54", |b| {
        b.iter(|| run_mlua_with(BENCH_USERDATA_METHODS, setup_userdata_mlua))
    });
    group.finish();
}

// ---------------------------------------------------------------------------
// Userdata borrow benchmark — exercises FromLuaBorrow with &T params
// ---------------------------------------------------------------------------

const BENCH_USERDATA_BORROW: &str = r#"
local total = 0
for i = 1, 500000 do
    total = total + geom:distance(a, b)
    total = total + geom:dot(a, b)
end
return total
"#;

struct Vec2 {
    x: f64,
    y: f64,
}

#[userdata]
impl Vec2 {
    fn type_name(&self) -> &'static str {
        "Vec2"
    }
}

struct GeomBorrow;

#[userdata]
impl GeomBorrow {
    fn type_name(&self) -> &'static str {
        "Geom"
    }

    #[lua_method]
    fn distance(&self, a: &Vec2, b: &Vec2) -> f64 {
        let dx = a.x - b.x;
        let dy = a.y - b.y;
        (dx * dx + dy * dy).sqrt()
    }

    #[lua_method]
    fn dot(&self, a: &Vec2, b: &Vec2) -> f64 {
        a.x * b.x + a.y * b.y
    }
}

struct GeomOwned;

#[userdata]
impl GeomOwned {
    fn type_name(&self) -> &'static str {
        "Geom"
    }

    #[lua_method]
    fn distance(&self, a: shingetsu::Ud<Vec2>, b: shingetsu::Ud<Vec2>) -> f64 {
        let dx = a.x - b.x;
        let dy = a.y - b.y;
        (dx * dx + dy * dy).sqrt()
    }

    #[lua_method]
    fn dot(&self, a: shingetsu::Ud<Vec2>, b: shingetsu::Ud<Vec2>) -> f64 {
        a.x * b.x + a.y * b.y
    }
}

fn setup_userdata_borrow_shingetsu(env: &GlobalEnv) {
    let geom = Arc::new(GeomBorrow);
    let a = Arc::new(Vec2 { x: 3.0, y: 4.0 });
    let b = Arc::new(Vec2 { x: 1.0, y: 2.0 });
    env.set_global("geom", Value::Userdata(geom as Arc<dyn shingetsu::Userdata>));
    env.set_global("a", Value::Userdata(a as Arc<dyn shingetsu::Userdata>));
    env.set_global("b", Value::Userdata(b as Arc<dyn shingetsu::Userdata>));
}

fn setup_userdata_owned_shingetsu(env: &GlobalEnv) {
    let geom = Arc::new(GeomOwned);
    let a = Arc::new(Vec2 { x: 3.0, y: 4.0 });
    let b = Arc::new(Vec2 { x: 1.0, y: 2.0 });
    env.set_global("geom", Value::Userdata(geom as Arc<dyn shingetsu::Userdata>));
    env.set_global("a", Value::Userdata(a as Arc<dyn shingetsu::Userdata>));
    env.set_global("b", Value::Userdata(b as Arc<dyn shingetsu::Userdata>));
}

fn setup_userdata_borrow_mlua(lua: &MLua) {
    let a_x: f64 = 3.0;
    let a_y: f64 = 4.0;
    let b_x: f64 = 1.0;
    let b_y: f64 = 2.0;

    let geom = lua.create_table().unwrap();
    geom.set(
        "distance",
        lua.create_function(move |_, (_self, a, b): (mlua::Value, mlua::Table, mlua::Table)| {
            let ax: f64 = a.get("x")?;
            let ay: f64 = a.get("y")?;
            let bx: f64 = b.get("x")?;
            let by: f64 = b.get("y")?;
            let dx = ax - bx;
            let dy = ay - by;
            Ok((dx * dx + dy * dy).sqrt())
        })
        .unwrap(),
    )
    .unwrap();
    geom.set(
        "dot",
        lua.create_function(move |_, (_self, a, b): (mlua::Value, mlua::Table, mlua::Table)| {
            let ax: f64 = a.get("x")?;
            let ay: f64 = a.get("y")?;
            let bx: f64 = b.get("x")?;
            let by: f64 = b.get("y")?;
            Ok(ax * bx + ay * by)
        })
        .unwrap(),
    )
    .unwrap();
    lua.globals().set("geom", geom).unwrap();

    let a = lua.create_table().unwrap();
    a.set("x", a_x).unwrap();
    a.set("y", a_y).unwrap();
    lua.globals().set("a", a).unwrap();

    let b = lua.create_table().unwrap();
    b.set("x", b_x).unwrap();
    b.set("y", b_y).unwrap();
    lua.globals().set("b", b).unwrap();
}

fn bench_userdata_borrow(c: &mut Criterion) {
    let mut group = c.benchmark_group("userdata_borrow");
    // Single iteration is ~317ms; cap to avoid long runs.
    // Override with: cargo bench -- --measurement-time=60 userdata_borrow
    cap_slow_benchmark(&mut group);
    group.bench_function("shingetsu_borrow", |b| {
        b.iter(|| run_shingetsu_with(BENCH_USERDATA_BORROW, setup_userdata_borrow_shingetsu))
    });
    group.bench_function("shingetsu_owned", |b| {
        b.iter(|| run_shingetsu_with(BENCH_USERDATA_BORROW, setup_userdata_owned_shingetsu))
    });
    group.bench_function("lua54", |b| {
        b.iter(|| run_mlua_with(BENCH_USERDATA_BORROW, setup_userdata_borrow_mlua))
    });
    group.finish();
}

const BENCH_LUA_CALL_CHAIN: &str = r#"
local function clamp(x, lo, hi)
    if x < lo then return lo end
    if x > hi then return hi end
    return x
end

local function score(a, b)
    return clamp(a * 3 + b, 0, 1000)
end

local function process(i)
    if i % 2 == 0 then
        return score(i, i + 1)
    else
        return score(i + 5, i - 1)
    end
end

local total = 0
for i = 1, 500000 do
    total = total + process(i)
end
return total
"#;

fn bench_lua_call_chain(c: &mut Criterion) {
    let mut group = c.benchmark_group("lua_call_chain");
    // Single iteration is ~278ms; cap to avoid long runs.
    // Override with: cargo bench -- --measurement-time=60 lua_call_chain
    cap_slow_benchmark(&mut group);
    group.bench_function("shingetsu", |b| {
        b.iter(|| run_shingetsu(BENCH_LUA_CALL_CHAIN))
    });
    group.bench_function("lua54", |b| b.iter(|| run_mlua(BENCH_LUA_CALL_CHAIN)));
    group.finish();
}

const BENCH_UPVALUE: &str = r#"
local function make_counter()
    local n = 0
    local function inc() n = n + 1 end
    local function get() return n end
    return inc, get
end

local inc, get = make_counter()
for i = 1, 2000000 do
    inc()
end
return get()
"#;

fn bench_upvalue(c: &mut Criterion) {
    let mut group = c.benchmark_group("upvalue");
    cap_slow_benchmark(&mut group);
    group.bench_function("shingetsu", |b| {
        b.iter(|| run_shingetsu(BENCH_UPVALUE))
    });
    group.bench_function("lua54", |b| b.iter(|| run_mlua(BENCH_UPVALUE)));
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
    bench_native_dispatch,
    bench_userdata_methods,
    bench_userdata_borrow,
    bench_lua_call_chain,
    bench_upvalue
);
criterion_main!(benches);
