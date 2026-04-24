mod common;

use common::{new_env, run_err_with_env, run_with_env};
use shingetsu::valuevec;

// Proc macro smoke tests
// ---------------------------------------------------------------------------

#[test]
fn derive_userdata_basic() {
    // #[derive(UserData)] generates a valid Userdata impl + downcast support.

    use shingetsu::UserData;
    use std::sync::Arc;

    #[derive(UserData)]
    struct Marker;

    let arc: Arc<dyn shingetsu::Userdata> = Arc::new(Marker);
    k9::assert_equal!(arc.type_name(), "Marker");
    // Downcast should succeed.
    assert!(arc.downcast_arc::<Marker>().is_ok());
}

#[tokio::test]
async fn userdata_macro_field_and_method() {
    // #[shingetsu::userdata] on an impl block wires __index dispatch.
    use shingetsu::{userdata, Function, Task, Value};
    use shingetsu_compiler::{CompileOptions, Compiler};
    use std::sync::Arc;

    struct Counter(i64);

    #[userdata]
    impl Counter {
        fn type_name(&self) -> &'static str {
            "Counter"
        }

        #[lua_field]
        fn value(&self) -> i64 {
            self.0
        }
    }

    let env = new_env();
    let counter: Arc<dyn shingetsu::Userdata> = Arc::new(Counter(42));
    env.set_global("counter", Value::Userdata(counter));

    let src = "return counter.value";
    let compiler = Compiler::new(
        CompileOptions {
            debug_info: false,
            source_name: "@test".into(),
            type_check: false,
        },
        Default::default(),
    );
    let bc = compiler.compile(src).await.expect("compile");
    let func = Function::lua(bc.top_level, vec![]);
    let results = Task::new(env, func, vec![]).await.expect("run");
    k9::assert_equal!(results[0], Value::Integer(42));
}

#[tokio::test]
async fn typeof_on_userdata_returns_host_type_name() {
    // typeof() surfaces the Userdata::type_name() value for userdata
    // values, whereas type() always returns "userdata".
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    struct Counter(#[allow(dead_code)] i64);

    #[userdata]
    impl Counter {
        fn type_name(&self) -> &'static str {
            "Counter"
        }
    }

    let env = new_env();
    env.set_global("c", Value::Userdata(Arc::new(Counter(1))));
    let res = run_with_env(env, "return type(c), typeof(c)").await;
    k9::assert_equal!(
        res,
        valuevec![Value::string("userdata"), Value::string("Counter")]
    );
}

#[tokio::test]
async fn module_macro_basic() {
    // #[shingetsu::module] generates build_module_table that registers functions.
    use shingetsu::{module, Function, Task, Value};
    use shingetsu_compiler::{CompileOptions, Compiler};

    #[module]
    mod testmod {
        #[function]
        fn add(a: i64, b: i64) -> i64 {
            a + b
        }
    }

    let env = new_env();
    testmod::register_global_module(&env).expect("register");

    let src = "return testmod.add(3, 4)";
    let compiler = Compiler::new(
        CompileOptions {
            debug_info: false,
            source_name: "@test".into(),
            type_check: false,
        },
        Default::default(),
    );
    let bc = compiler.compile(src).await.expect("compile");
    let func = Function::lua(bc.top_level, vec![]);
    let results = Task::new(env, func, vec![]).await.expect("run");
    k9::assert_equal!(results[0], Value::Integer(7));
}

// ---------------------------------------------------------------------------
// Userdata macro: field getter with rename
// ---------------------------------------------------------------------------

#[tokio::test]
async fn userdata_macro_field_rename() {
    // #[lua_field(rename = "luaName")] maps the Lua key to a different name.
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    struct Point(i64, i64);

    #[userdata]
    impl Point {
        #[lua_field(rename = "x")]
        fn get_x(&self) -> i64 {
            self.0
        }

        #[lua_field(rename = "y")]
        fn get_y(&self) -> i64 {
            self.1
        }
    }

    let env = new_env();
    env.set_global("pt", Value::Userdata(Arc::new(Point(3, 7))));
    let res = run_with_env(env, "return pt.x + pt.y").await;
    k9::assert_equal!(res[0], Value::Integer(10));
}

// ---------------------------------------------------------------------------
// Userdata macro: field setter via set_ prefix
// ---------------------------------------------------------------------------

#[tokio::test]
async fn userdata_macro_field_setter() {
    // A fn named set_<field> is detected as a setter; __newindex dispatches it.
    use shingetsu::{userdata, Value};
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::Arc;

    struct Counter(AtomicI64);

    #[userdata]
    impl Counter {
        #[lua_field]
        fn value(&self) -> i64 {
            self.0.load(Ordering::Relaxed)
        }

        #[lua_field]
        fn set_value(&self, v: i64) {
            self.0.store(v, Ordering::Relaxed);
        }
    }

    let env = new_env();
    env.set_global("c", Value::Userdata(Arc::new(Counter(AtomicI64::new(0)))));
    let res = run_with_env(env, "c.value = 99; return c.value").await;
    k9::assert_equal!(res[0], Value::Integer(99));
}

#[tokio::test]
async fn validate_args_field_setter_rejects_wrong_type() {
    // Inline type checks in gen_call_body catch type mismatches for
    // field setter parameters (which don't go through validate_args).
    use shingetsu::{userdata, Value};
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::Arc;

    struct Counter(AtomicI64);

    #[userdata]
    impl Counter {
        #[lua_field]
        fn value(&self) -> i64 {
            self.0.load(Ordering::Relaxed)
        }

        #[lua_field]
        fn set_value(&self, v: i64) {
            self.0.store(v, Ordering::Relaxed);
        }
    }

    let env = new_env();
    env.set_global("c", Value::Userdata(Arc::new(Counter(AtomicI64::new(0)))));
    let res = run_with_env(
        env,
        "local ok, err = pcall(function() c.value = 'oops' end)\n\
         return ok, err",
    )
    .await;
    k9::assert_equal!(
        res,
        valuevec![
            Value::Boolean(false),
            Value::string(
                "bad value in assignment to 'Counter.value' (number expected, got string)"
            ),
        ]
    );
}

// ---------------------------------------------------------------------------
// Userdata macro: method with &self receiver and a parameter
// ---------------------------------------------------------------------------

#[tokio::test]
async fn userdata_macro_method_ref_self() {
    // #[lua_method] with &self — the object is skipped from the Lua arg list.
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_method]
        fn multiply(&self, factor: i64) -> i64 {
            self.0 * factor
        }
    }

    let env = new_env();
    env.set_global("n", Value::Userdata(Arc::new(Num(7))));
    // obj:method(arg) desugars to obj.method(obj, arg)
    let res = run_with_env(env, "return n:multiply(6)").await;
    k9::assert_equal!(res[0], Value::Integer(42));
}

// ---------------------------------------------------------------------------
// Userdata macro: method with Arc<Self> receiver
// ---------------------------------------------------------------------------

#[tokio::test]
async fn userdata_macro_method_arc_self() {
    // #[lua_method] where self is Arc<Self> — passes the Arc directly.
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_method]
        fn doubled(self: Arc<Self>) -> i64 {
            self.0 * 2
        }
    }

    let env = new_env();
    env.set_global("n", Value::Userdata(Arc::new(Num(21))));
    let res = run_with_env(env, "return n:doubled()").await;
    k9::assert_equal!(res[0], Value::Integer(42));
}

// ---------------------------------------------------------------------------
// Userdata macro: method returning Result
// ---------------------------------------------------------------------------

#[tokio::test]
async fn userdata_macro_method_result_ok() {
    // A method with Result return — Ok path propagates the value normally.
    use shingetsu::{userdata, Value, VmError};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_method]
        fn checked_div(&self, divisor: i64) -> Result<i64, VmError> {
            if divisor == 0 {
                Err(VmError::HostError {
                    name: "checked_div".to_owned(),
                    source: "division by zero".into(),
                })
            } else {
                Ok(self.0 / divisor)
            }
        }
    }

    let env = new_env();
    env.set_global("n", Value::Userdata(Arc::new(Num(42))));
    let res = run_with_env(env, "return n:checked_div(6)").await;
    k9::assert_equal!(res[0], Value::Integer(7));
}

#[tokio::test]
async fn userdata_macro_method_result_err() {
    // A method with Result return — Err path surfaces as a Lua error.
    use shingetsu::{userdata, Value, VmError};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_method]
        fn checked_div(&self, divisor: i64) -> Result<i64, VmError> {
            if divisor == 0 {
                Err(VmError::HostError {
                    name: "checked_div".to_owned(),
                    source: "division by zero".into(),
                })
            } else {
                Ok(self.0 / divisor)
            }
        }
    }

    use shingetsu::{Function, Task};
    use shingetsu_compiler::{CompileOptions, Compiler};

    let env = new_env();
    env.set_global("n", Value::Userdata(Arc::new(Num(42))));
    let compiler = Compiler::new(
        CompileOptions {
            debug_info: false,
            source_name: "@test".into(),
            type_check: false,
        },
        Default::default(),
    );
    let bc = compiler
        .compile("return n:checked_div(0)")
        .await
        .expect("compile");
    let func = Function::lua(bc.top_level, vec![]);
    let err = Task::new(env, func, vec![]).await.unwrap_err();
    k9::assert_equal!(err.to_string(), "error in 'checked_div': division by zero");
}

// ---------------------------------------------------------------------------
// Userdata macro: method with CallContext parameter
// ---------------------------------------------------------------------------

#[tokio::test]
async fn userdata_macro_method_callcontext() {
    // A CallContext parameter is injected from the call site, not from Lua args.
    use shingetsu::{userdata, CallContext, Value};
    use std::sync::Arc;

    struct Doubler;

    #[userdata]
    impl Doubler {
        #[lua_method]
        fn run(&self, _ctx: CallContext, n: i64) -> i64 {
            n * 2
        }
    }

    let env = new_env();
    env.set_global("d", Value::Userdata(Arc::new(Doubler)));
    let res = run_with_env(env, "return d:run(21)").await;
    k9::assert_equal!(res[0], Value::Integer(42));
}

// ---------------------------------------------------------------------------
// Userdata macro: method with Variadic parameter
// ---------------------------------------------------------------------------

#[tokio::test]
async fn userdata_macro_method_variadic() {
    // A Variadic parameter collects all remaining Lua args into a Vec.
    use shingetsu::{userdata, Value, Variadic};
    use std::sync::Arc;

    struct Summer;

    #[userdata]
    impl Summer {
        #[lua_method]
        fn sum(&self, args: Variadic) -> i64 {
            args.0
                .iter()
                .filter_map(|v| match v {
                    Value::Integer(n) => Some(*n),
                    _ => None,
                })
                .sum()
        }
    }

    let env = new_env();
    env.set_global("s", Value::Userdata(Arc::new(Summer)));
    let res = run_with_env(env, "return s:sum(1, 2, 3, 4)").await;
    k9::assert_equal!(res[0], Value::Integer(10));
}

// ---------------------------------------------------------------------------
// Userdata macro: __tostring metamethod via tostring() builtin
// ---------------------------------------------------------------------------

#[tokio::test]
async fn userdata_macro_metamethod_tostring() {
    // #[lua_metamethod(ToString)] is dispatched by the tostring() global.
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    struct Named(String);

    #[userdata]
    impl Named {
        #[lua_metamethod(ToString)]
        fn to_str(&self) -> String {
            self.0.clone()
        }
    }

    let env = new_env();
    env.set_global("obj", Value::Userdata(Arc::new(Named("hello".into()))));
    let res = run_with_env(env, "return tostring(obj)").await;
    k9::assert_equal!(res[0], Value::string("hello"));
}

// ---------------------------------------------------------------------------
// Userdata macro: binary metamethod dispatched directly
// ---------------------------------------------------------------------------

#[tokio::test]
async fn userdata_macro_metamethod_binary_dispatch() {
    // #[lua_metamethod(Add)] — test the dispatch mechanism directly.
    // TODO: once get_arith_metamethod in task.rs is extended to handle
    // Value::Userdata, replace this with a Lua `a + b` test instead.
    // See the TODO comment on get_arith_metamethod in shingetsu-vm/src/task.rs.
    use shingetsu::{userdata, CallContext, Value};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_metamethod(Add)]
        fn add_mm(&self, rhs: i64) -> i64 {
            self.0 + rhs
        }
    }

    let env = new_env();
    let obj: Arc<dyn shingetsu::Userdata> = Arc::new(Num(10));
    let ctx = CallContext {
        global: env,
        call_stack: Arc::new(vec![]),
        native_name: None,
    };
    let result = Arc::clone(&obj)
        .dispatch(ctx, "__add", vec![Value::Userdata(obj), Value::Integer(5)])
        .await
        .expect("dispatch");
    k9::assert_equal!(result[0], Value::Integer(15));
}

#[tokio::test]
async fn validate_args_metamethod_rejects_wrong_type() {
    // Inline type checks in gen_call_body catch type mismatches for
    // metamethod parameters (which don't go through validate_args).
    use shingetsu::{userdata, CallContext, Value};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_metamethod(Add)]
        fn add_mm(&self, rhs: i64) -> i64 {
            self.0 + rhs
        }
    }

    let env = new_env();
    let obj: Arc<dyn shingetsu::Userdata> = Arc::new(Num(10));
    let ctx = CallContext {
        global: env,
        call_stack: Arc::new(vec![]),
        native_name: None,
    };
    let err = Arc::clone(&obj)
        .dispatch(
            ctx,
            "__add",
            vec![Value::Userdata(obj), Value::string("oops")],
        )
        .await
        .unwrap_err();
    k9::assert_equal!(
        err.to_string(),
        "bad argument #1 to 'Num:__add' (number expected, got string)"
    );
}

// ---------------------------------------------------------------------------
// Module macro: function returning Result (Ok path)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn module_macro_result_return() {
    use shingetsu::{module, Value};

    #[module]
    mod mathmod {
        use shingetsu::VmError;

        #[function]
        fn checked_sqrt(n: f64) -> Result<f64, VmError> {
            if n < 0.0 {
                Err(VmError::HostError {
                    name: "checked_sqrt".to_owned(),
                    source: "negative input".into(),
                })
            } else {
                Ok(n.sqrt())
            }
        }
    }

    let env = new_env();
    mathmod::register_global_module(&env).expect("register");
    let res = run_with_env(env, "return mathmod.checked_sqrt(4.0)").await;
    k9::assert_equal!(res[0], Value::Float(2.0));
}

// ---------------------------------------------------------------------------
// Module macro: async function
// ---------------------------------------------------------------------------

#[tokio::test]
async fn module_macro_async_fn() {
    use shingetsu::{module, Value};

    #[module]
    mod asyncmod {
        #[function]
        async fn async_double(n: i64) -> i64 {
            n * 2
        }
    }

    let env = new_env();
    asyncmod::register_global_module(&env).expect("register");
    let res = run_with_env(env, "return asyncmod.async_double(21)").await;
    k9::assert_equal!(res[0], Value::Integer(42));
}

// ---------------------------------------------------------------------------
// Module macro: function with CallContext parameter
// ---------------------------------------------------------------------------

#[tokio::test]
async fn module_macro_callcontext() {
    use shingetsu::{module, Value};

    #[module]
    mod ctxmod {
        use shingetsu::CallContext;

        #[function]
        fn passthrough(_ctx: CallContext, n: i64) -> i64 {
            n
        }
    }

    let env = new_env();
    ctxmod::register_global_module(&env).expect("register");
    let res = run_with_env(env, "return ctxmod.passthrough(99)").await;
    k9::assert_equal!(res[0], Value::Integer(99));
}

// ---------------------------------------------------------------------------
// Module macro: function with Variadic parameter
// ---------------------------------------------------------------------------

#[tokio::test]
async fn module_macro_variadic() {
    use shingetsu::{module, Value};

    #[module]
    mod varmod {
        use shingetsu::{Value, Variadic};

        #[function]
        fn sum_all(args: Variadic) -> i64 {
            args.0
                .iter()
                .filter_map(|v| match v {
                    Value::Integer(n) => Some(*n),
                    _ => None,
                })
                .sum()
        }
    }

    let env = new_env();
    varmod::register_global_module(&env).expect("register");
    let res = run_with_env(env, "return varmod.sum_all(10, 20, 12)").await;
    k9::assert_equal!(res[0], Value::Integer(42));
}

// ---------------------------------------------------------------------------
// Module macro: eager field
// ---------------------------------------------------------------------------

#[tokio::test]
async fn module_macro_eager_field() {
    // #[field] is called once at table construction; the result is stored eagerly.
    use shingetsu::{module, Value};

    #[module]
    mod constmod {
        #[field]
        fn magic() -> i64 {
            42
        }
    }

    let env = new_env();
    constmod::register_global_module(&env).expect("register");
    let res = run_with_env(env, "return constmod.magic").await;
    k9::assert_equal!(res[0], Value::Integer(42));
}

// ---------------------------------------------------------------------------
// Module macro: function rename
// ---------------------------------------------------------------------------

#[tokio::test]
async fn module_macro_function_rename() {
    // #[function(rename = "luaName")] exposes the function under a different key.
    use shingetsu::{module, Value};

    #[module]
    mod renmod {
        #[function(rename = "doThing")]
        fn do_thing(n: i64) -> i64 {
            n + 1
        }
    }

    let env = new_env();
    renmod::register_global_module(&env).expect("register");
    let res = run_with_env(env, "return renmod.doThing(5)").await;
    k9::assert_equal!(res[0], Value::Integer(6));
}

// ---------------------------------------------------------------------------
// Module macro: module name option overrides global key
// ---------------------------------------------------------------------------

#[tokio::test]
async fn module_macro_name_option() {
    // #[module(name = "luaName")] controls the key used in set_global.
    use shingetsu::{module, Value};

    #[module(name = "myMod")]
    mod internal {
        #[function]
        fn hello() -> i64 {
            1
        }
    }

    let env = new_env();
    internal::register_global_module(&env).expect("register");
    // The Rust mod is named `internal` but the Lua global is `myMod`.
    let res = run_with_env(env, "return myMod.hello()").await;
    k9::assert_equal!(res[0], Value::Integer(1));
}

// ---------------------------------------------------------------------------
// Userdata macro: get_ prefix is stripped automatically for field names
// ---------------------------------------------------------------------------

#[tokio::test]
async fn userdata_macro_field_get_prefix() {
    // fn get_<name> maps to Lua field "<name>" without requiring rename =.
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    struct Rect {
        w: i64,
        h: i64,
    }

    #[userdata]
    impl Rect {
        #[lua_field]
        fn get_width(&self) -> i64 {
            self.w
        }

        #[lua_field]
        fn get_height(&self) -> i64 {
            self.h
        }
    }

    let env = new_env();
    env.set_global("r", Value::Userdata(Arc::new(Rect { w: 4, h: 6 })));
    // Fields are "width" and "height", not "get_width" / "get_height".
    let res = run_with_env(env, "return r.width * r.height").await;
    k9::assert_equal!(res[0], Value::Integer(24));
}

// ---------------------------------------------------------------------------
// Userdata macro: set_ prefix extraction and setter dispatch
// ---------------------------------------------------------------------------

#[tokio::test]
async fn userdata_macro_field_set_prefix() {
    // fn set_<name> maps to Lua field "<name>" for __newindex, matching the
    // getter derived from fn get_<name> or fn <name>.
    use shingetsu::{userdata, Value};
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::Arc;

    struct Cube(AtomicI64);

    #[userdata]
    impl Cube {
        #[lua_field]
        fn get_side(&self) -> i64 {
            self.0.load(Ordering::Relaxed)
        }

        #[lua_field]
        fn set_side(&self, v: i64) {
            self.0.store(v, Ordering::Relaxed);
        }
    }

    let env = new_env();
    env.set_global("b", Value::Userdata(Arc::new(Cube(AtomicI64::new(0)))));
    // Both fn get_side and fn set_side map to the Lua field "side".
    let res = run_with_env(env, "b.side = 5; return b.side").await;
    k9::assert_equal!(res[0], Value::Integer(5));
}

// ---------------------------------------------------------------------------
// Result<T, E> where E: Into<VmError> — custom error type conversion
// ---------------------------------------------------------------------------

#[tokio::test]
async fn module_macro_result_custom_error() {
    // Demonstrates that Result<T, E> works when E: Into<VmError>, not just
    // when E is VmError directly.  ParseError and its From impl are defined
    // inside the module so they are in scope for the generated wrapper code.
    use shingetsu::{module, Value};

    #[module]
    mod parsemod {
        use shingetsu::VmError;

        #[derive(Debug)]
        pub struct ParseError(pub String);

        impl std::fmt::Display for ParseError {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl std::error::Error for ParseError {}

        impl From<ParseError> for VmError {
            fn from(e: ParseError) -> VmError {
                VmError::HostError {
                    name: "parse_int".to_owned(),
                    source: Box::new(e),
                }
            }
        }

        #[function]
        fn parse_int(s: String) -> Result<i64, ParseError> {
            s.parse::<i64>().map_err(|e| ParseError(e.to_string()))
        }
    }

    let env = new_env();
    parsemod::register_global_module(&env).expect("register");

    // Ok path: valid integer string.
    let res = run_with_env(env.clone(), "return parsemod.parse_int('42')").await;
    k9::assert_equal!(res[0], Value::Integer(42));

    // Err path: non-integer string surfaces as VmError.
    use shingetsu::{Function, Task};
    use shingetsu_compiler::{CompileOptions, Compiler};
    let compiler = Compiler::new(
        CompileOptions {
            debug_info: false,
            source_name: "@test".into(),
            type_check: false,
        },
        Default::default(),
    );
    let bc = compiler
        .compile("return parsemod.parse_int('nope')")
        .await
        .expect("compile");
    let func = Function::lua(bc.top_level, vec![]);
    let err = Task::new(env, func, vec![]).await.unwrap_err();
    k9::assert_equal!(
        err.to_string(),
        "error in 'parse_int': invalid digit found in string"
    );
}

// ---------------------------------------------------------------------------
// Module macro: `this` table parameter (colon-call passes module table)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn module_macro_this_param() {
    // When a module function is called with `:` syntax, Lua passes the module
    // table itself as the first argument.  Declaring `this: Table` captures it.
    use shingetsu::{module, Value};

    #[module(name = "tmod")]
    mod tmod_impl {
        use shingetsu::{Table, Value};

        /// An eager constant field baked into the table at construction time.
        #[field]
        fn version() -> i64 {
            99
        }

        /// Reads a field from the module table passed as `this`.
        #[function]
        fn read_version(this: Table) -> i64 {
            match this
                .raw_get(&Value::string("version"))
                .unwrap_or(Value::Nil)
            {
                Value::Integer(n) => n,
                _ => -1,
            }
        }
    }

    let env = new_env();
    tmod_impl::register_global_module(&env).expect("register");
    // tmod:read_version() desugars to tmod.read_version(tmod); `this` == tmod.
    let res = run_with_env(env, "return tmod:read_version()").await;
    k9::assert_equal!(res[0], Value::Integer(99));
}

// ---------------------------------------------------------------------------
// Userdata macro: lua_type_info returns structural table type
// ---------------------------------------------------------------------------

#[test]
fn userdata_lua_type_info_methods_and_fields() {
    use shingetsu::{userdata, FunctionLuaType, LuaType, TableLuaType, Userdata};

    struct Counter(#[allow(dead_code)] i64);

    #[userdata]
    impl Counter {
        #[lua_field]
        fn value(&self) -> i64 {
            self.0
        }

        #[lua_method]
        fn increment(&self, amount: i64) -> i64 {
            self.0 + amount
        }
    }

    let c = Counter(0);
    let ty = c.lua_type_info();
    k9::assert_equal!(
        ty,
        LuaType::Table(Box::new(TableLuaType {
            fields: vec![
                (
                    shingetsu_vm::Bytes::from("increment"),
                    LuaType::Function(Box::new(FunctionLuaType {
                        type_params: vec![],
                        params: vec![(Some(shingetsu_vm::Bytes::from("amount")), LuaType::Number),],
                        variadic: None,
                        returns: vec![LuaType::Number],
                        is_method: true,
                        inferred_unannotated: false,
                    })),
                ),
                (shingetsu_vm::Bytes::from("value"), LuaType::Any,),
            ],
            indexer: None,
        }))
    );
}

#[test]
fn userdata_lua_type_info_default_is_named() {
    use shingetsu::{LuaType, UserData, Userdata};

    #[derive(UserData)]
    struct Simple;

    let s = Simple;
    k9::assert_equal!(
        s.lua_type_info(),
        LuaType::Named(shingetsu_vm::Bytes::from("Simple"))
    );
}

#[test]
fn userdata_lua_type_info_via_set_global() {
    use shingetsu::{userdata, FunctionLuaType, LuaType, TableLuaType, Value};
    use std::sync::Arc;

    struct Greeter;

    #[userdata]
    impl Greeter {
        #[lua_method]
        fn greet(&self, name: String) -> String {
            format!("hello {name}")
        }
    }

    let env = new_env();
    env.set_global("g", Value::Userdata(Arc::new(Greeter)));
    let map = env.global_type_map();
    k9::assert_equal!(
        map.get(b"g"),
        Some(&LuaType::Table(Box::new(TableLuaType {
            fields: vec![(
                shingetsu_vm::Bytes::from("greet"),
                LuaType::Function(Box::new(FunctionLuaType {
                    type_params: vec![],
                    params: vec![(Some(shingetsu_vm::Bytes::from("name")), LuaType::String),],
                    variadic: None,
                    returns: vec![LuaType::String],
                    is_method: true,
                    inferred_unannotated: false,
                })),
            )],
            indexer: None,
        })))
    );
}

// ---------------------------------------------------------------------------
// Userdata macro: __len metamethod via the # operator
// ---------------------------------------------------------------------------

#[tokio::test]
async fn userdata_len_metamethod() {
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    struct Items(Vec<String>);

    #[userdata]
    impl Items {
        #[lua_metamethod(Len)]
        fn len(&self) -> i64 {
            self.0.len() as i64
        }
    }

    let env = new_env();
    env.set_global(
        "items",
        Value::Userdata(Arc::new(Items(vec!["a".into(), "b".into(), "c".into()]))),
    );
    let res = run_with_env(env, "return #items").await;
    k9::assert_equal!(res, valuevec![Value::Integer(3)]);
}

#[tokio::test]
async fn userdata_len_no_metamethod_errors() {
    use shingetsu::diagnostic::{render_runtime_error, RenderStyle};
    use shingetsu::{userdata, Function, Task, Value};
    use shingetsu_compiler::{CompileOptions, Compiler};
    use std::sync::Arc;

    struct Plain;

    #[userdata]
    impl Plain {}

    let env = new_env();
    env.set_global("obj", Value::Userdata(Arc::new(Plain)));
    let src = "return #obj";
    let compiler = Compiler::new(
        CompileOptions {
            debug_info: true,
            source_name: "@test.lua".into(),
            type_check: false,
        },
        Default::default(),
    );
    let bc = compiler.compile(src).await.expect("compile");
    let func = Function::lua(bc.top_level, vec![]);
    let err = Task::new(env, func, vec![]).await.unwrap_err();
    let rendered = render_runtime_error(&err, RenderStyle::Plain);
    k9::assert_equal!(
        rendered,
        "\
error: error in 'Plain:__len': metamethod '__len' not implemented for 'Plain'
 --> test.lua:1:8
  |
1 | return #obj
  |        ^^^^ error in 'Plain:__len': metamethod '__len' not implemented for 'Plain'
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn userdata_len_non_integer_return() {
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    struct Weird;

    #[userdata]
    impl Weird {
        #[lua_metamethod(Len)]
        fn len(&self) -> String {
            "not a number".to_string()
        }
    }

    let env = new_env();
    env.set_global("w", Value::Userdata(Arc::new(Weird)));
    let res = run_with_env(env, "return #w").await;
    k9::assert_equal!(res, valuevec![Value::string("not a number")]);
}

#[tokio::test]
async fn userdata_len_in_expression() {
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    struct Items(Vec<String>);

    #[userdata]
    impl Items {
        #[lua_metamethod(Len)]
        fn len(&self) -> i64 {
            self.0.len() as i64
        }
    }

    let env = new_env();
    env.set_global(
        "items",
        Value::Userdata(Arc::new(Items(vec!["a".into(), "b".into()]))),
    );
    let res = run_with_env(env, "return #items + 10").await;
    k9::assert_equal!(res, valuevec![Value::Integer(12)]);
}

#[tokio::test]
async fn userdata_len_error_propagates() {
    use shingetsu::diagnostic::{render_runtime_error, RenderStyle};
    use shingetsu::{userdata, Function, Task, Value, VmError};
    use shingetsu_compiler::{CompileOptions, Compiler};
    use std::sync::Arc;

    struct Broken;

    #[userdata]
    impl Broken {
        #[lua_metamethod(Len)]
        fn len(&self) -> Result<i64, VmError> {
            Err(VmError::LuaError {
                display: "length unavailable".into(),
                value: Value::string("length unavailable"),
            })
        }
    }

    let env = new_env();
    env.set_global("b", Value::Userdata(Arc::new(Broken)));
    let src = "return #b";
    let compiler = Compiler::new(
        CompileOptions {
            debug_info: true,
            source_name: "@test.lua".into(),
            type_check: false,
        },
        Default::default(),
    );
    let bc = compiler.compile(src).await.expect("compile");
    let func = Function::lua(bc.top_level, vec![]);
    let err = Task::new(env, func, vec![]).await.unwrap_err();
    let rendered = render_runtime_error(&err, RenderStyle::Plain);
    k9::assert_equal!(
        rendered,
        "\
error: length unavailable
 --> test.lua:1:8
  |
1 | return #b
  |        ^^ length unavailable
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

// ---------------------------------------------------------------------------
// Userdata arithmetic metamethods dispatched by the VM
// ---------------------------------------------------------------------------

#[tokio::test]
async fn userdata_arith_add_via_vm() {
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_metamethod(Add)]
        fn add_mm(&self, rhs: i64) -> i64 {
            self.0 + rhs
        }
    }

    let env = new_env();
    env.set_global("obj", Value::Userdata(Arc::new(Num(10))));
    let result = run_with_env(env, "return obj + 5").await;
    k9::assert_equal!(result, valuevec![Value::Integer(15)]);
}

#[tokio::test]
async fn userdata_arith_add_rhs_via_vm() {
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_metamethod(Add)]
        fn add_mm(&self, rhs: i64) -> i64 {
            self.0 + rhs
        }
    }

    let env = new_env();
    env.set_global("obj", Value::Userdata(Arc::new(Num(10))));
    let result = run_with_env(env, "return 5 + obj").await;
    k9::assert_equal!(result, valuevec![Value::Integer(15)]);
}

#[tokio::test]
async fn userdata_arith_sub_via_vm() {
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_metamethod(Sub)]
        fn sub_mm(&self, rhs: i64) -> i64 {
            self.0 - rhs
        }
    }

    let env = new_env();
    env.set_global("obj", Value::Userdata(Arc::new(Num(10))));
    let result = run_with_env(env, "return obj - 3").await;
    k9::assert_equal!(result, valuevec![Value::Integer(7)]);
}

#[tokio::test]
async fn userdata_arith_mul_via_vm() {
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_metamethod(Mul)]
        fn mul_mm(&self, rhs: i64) -> i64 {
            self.0 * rhs
        }
    }

    let env = new_env();
    env.set_global("obj", Value::Userdata(Arc::new(Num(10))));
    let result = run_with_env(env, "return obj * 3").await;
    k9::assert_equal!(result, valuevec![Value::Integer(30)]);
}

#[tokio::test]
async fn userdata_comparison_lt_via_vm() {
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_metamethod(Lt)]
        fn lt_mm(&self, rhs: i64) -> bool {
            self.0 < rhs
        }
    }

    let env = new_env();
    env.set_global("obj", Value::Userdata(Arc::new(Num(5))));
    let result = run_with_env(env, "return obj < 10, obj < 3").await;
    k9::assert_equal!(
        result,
        valuevec![Value::Boolean(true), Value::Boolean(false)]
    );
}

#[tokio::test]
async fn userdata_concat_via_vm() {
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    struct Label(String);

    #[userdata]
    impl Label {
        #[lua_metamethod(Concat)]
        fn concat_mm(&self, rhs: String) -> String {
            format!("{}{rhs}", self.0)
        }
    }

    let env = new_env();
    env.set_global("lbl", Value::Userdata(Arc::new(Label("hello".into()))));
    let result = run_with_env(env, r#"return lbl .. " world""#).await;
    k9::assert_equal!(result, valuevec![Value::string("hello world")]);
}

#[tokio::test]
async fn userdata_unm_via_vm() {
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_metamethod(Unm)]
        fn unm_mm(&self) -> i64 {
            -self.0
        }
    }

    let env = new_env();
    env.set_global("obj", Value::Userdata(Arc::new(Num(7))));
    let result = run_with_env(env, "return -obj").await;
    k9::assert_equal!(result, valuevec![Value::Integer(-7)]);
}

#[tokio::test]
async fn userdata_bnot_via_vm() {
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_metamethod(BNot)]
        fn bnot_mm(&self) -> i64 {
            !self.0
        }
    }

    let env = new_env();
    env.set_global("obj", Value::Userdata(Arc::new(Num(0))));
    let result = run_with_env(env, "return ~obj").await;
    k9::assert_equal!(result, valuevec![Value::Integer(-1)]);
}

#[tokio::test]
async fn userdata_band_via_vm() {
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    struct Bits(i64);

    #[userdata]
    impl Bits {
        #[lua_metamethod(BAnd)]
        fn band_mm(&self, rhs: i64) -> i64 {
            self.0 & rhs
        }
    }

    let env = new_env();
    env.set_global("obj", Value::Userdata(Arc::new(Bits(0xFF))));
    let result = run_with_env(env, "return obj & 0x0F").await;
    k9::assert_equal!(result, valuevec![Value::Integer(0x0F)]);
}

#[tokio::test]
async fn userdata_shl_via_vm() {
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    struct Bits(i64);

    #[userdata]
    impl Bits {
        #[lua_metamethod(Shl)]
        fn shl_mm(&self, rhs: i64) -> i64 {
            self.0 << rhs
        }
    }

    let env = new_env();
    env.set_global("obj", Value::Userdata(Arc::new(Bits(1))));
    let result = run_with_env(env, "return obj << 4").await;
    k9::assert_equal!(result, valuevec![Value::Integer(16)]);
}

#[tokio::test]
async fn userdata_le_via_vm() {
    use shingetsu::{userdata, BinOpSide, Value};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_metamethod(Le)]
        fn le_mm(&self, other: BinOpSide<i64>) -> bool {
            other.impl_le(self.0)
        }
    }

    let env = new_env();
    env.set_global("obj", Value::Userdata(Arc::new(Num(5))));
    let result = run_with_env(env, "return obj <= 5, obj <= 4").await;
    k9::assert_equal!(
        result,
        valuevec![Value::Boolean(true), Value::Boolean(false)]
    );
}

#[tokio::test]
async fn userdata_gt_via_vm() {
    use shingetsu::{userdata, BinOpSide, Value};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_metamethod(Lt)]
        fn lt_mm(&self, other: BinOpSide<i64>) -> bool {
            other.impl_lt(self.0)
        }
    }

    let env = new_env();
    env.set_global("obj", Value::Userdata(Arc::new(Num(5))));
    // Gt swaps operands: `obj > 3` becomes `__lt(3, obj)`.
    // BinOpSide ensures correct comparison regardless of operand order.
    let result = run_with_env(env, "return obj > 3, obj > 10").await;
    k9::assert_equal!(
        result,
        valuevec![Value::Boolean(true), Value::Boolean(false)]
    );
}

#[tokio::test]
async fn userdata_ge_via_vm() {
    use shingetsu::{userdata, BinOpSide, Value};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_metamethod(Le)]
        fn le_mm(&self, other: BinOpSide<i64>) -> bool {
            other.impl_le(self.0)
        }
    }

    let env = new_env();
    env.set_global("obj", Value::Userdata(Arc::new(Num(5))));
    // Ge swaps operands: `obj >= 5` becomes `__le(5, obj)`.
    // BinOpSide ensures correct comparison regardless of operand order.
    let result = run_with_env(env, "return obj >= 5, obj >= 6").await;
    k9::assert_equal!(
        result,
        valuevec![Value::Boolean(true), Value::Boolean(false)]
    );
}

#[tokio::test]
async fn table_metamethod_takes_priority_over_userdata() {
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_metamethod(Add)]
        fn add_mm(&self, rhs: i64) -> i64 {
            self.0 + rhs
        }
    }

    let env = new_env();
    env.set_global("ud", Value::Userdata(Arc::new(Num(100))));
    // Table with __add on LHS should win over userdata on RHS
    let result = run_with_env(
        env,
        "local mt = { __add = function(a, b) return 999 end }
local t = setmetatable({}, mt)
return t + ud",
    )
    .await;
    k9::assert_equal!(result, valuevec![Value::Integer(999)]);
}

#[tokio::test]
async fn userdata_sub_binopside_via_vm() {
    use shingetsu::{userdata, BinOpSide, Value};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_metamethod(Sub)]
        fn sub_mm(&self, other: BinOpSide<i64>) -> i64 {
            other.impl_sub(self.0)
        }
    }

    let env = new_env();
    env.set_global("obj", Value::Userdata(Arc::new(Num(10))));
    // obj - 3 = 10 - 3 = 7 (self on left)
    // 3 - obj = 3 - 10 = -7 (self on right)
    let result = run_with_env(env, "return obj - 3, 3 - obj").await;
    k9::assert_equal!(result, valuevec![Value::Integer(7), Value::Integer(-7)]);
}

#[tokio::test]
async fn userdata_sub_apply_via_vm() {
    use shingetsu::{userdata, BinOpSide, Value};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_metamethod(Sub)]
        fn sub_mm(&self, other: BinOpSide<i64>) -> i64 {
            other.apply(self.0, |lhs, rhs| lhs - rhs)
        }
    }

    let env = new_env();
    env.set_global("obj", Value::Userdata(Arc::new(Num(10))));
    let result = run_with_env(env, "return obj - 3, 3 - obj").await;
    k9::assert_equal!(result, valuevec![Value::Integer(7), Value::Integer(-7)]);
}

#[tokio::test]
async fn userdata_lt_both_directions_via_vm() {
    use shingetsu::{userdata, BinOpSide, Value};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_metamethod(Lt)]
        fn lt_mm(&self, other: BinOpSide<i64>) -> bool {
            other.impl_lt(self.0)
        }
    }

    let env = new_env();
    env.set_global("obj", Value::Userdata(Arc::new(Num(5))));
    // obj < 10 = true (self on left, other=10 RightOfOperator)
    // obj < 3 = false (self on left, other=3 RightOfOperator)
    // obj > 3 = true  (swapped to __lt(3, obj), other=3 LeftOfOperator)
    // obj > 10 = false (swapped to __lt(10, obj), other=10 LeftOfOperator)
    let result = run_with_env(env, "return obj < 10, obj < 3, obj > 3, obj > 10").await;
    k9::assert_equal!(
        result,
        valuevec![
            Value::Boolean(true),
            Value::Boolean(false),
            Value::Boolean(true),
            Value::Boolean(false),
        ]
    );
}

#[tokio::test]
async fn userdata_div_binopside_via_vm() {
    use shingetsu::{userdata, BinOpSide, Value};
    use std::sync::Arc;

    struct Num(f64);

    #[userdata]
    impl Num {
        #[lua_metamethod(Div)]
        fn div_mm(&self, other: BinOpSide<f64>) -> f64 {
            other.impl_div(self.0)
        }
    }

    let env = new_env();
    env.set_global("obj", Value::Userdata(Arc::new(Num(10.0))));
    let result = run_with_env(env, "return obj / 2, 100 / obj").await;
    k9::assert_equal!(result, valuevec![Value::Float(5.0), Value::Float(10.0)]);
}

#[tokio::test]
async fn userdata_mod_binopside_via_vm() {
    use shingetsu::{userdata, BinOpSide, Value};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_metamethod(Mod)]
        fn mod_mm(&self, other: BinOpSide<i64>) -> i64 {
            other.impl_rem(self.0)
        }
    }

    let env = new_env();
    env.set_global("obj", Value::Userdata(Arc::new(Num(10))));
    let result = run_with_env(env, "return obj % 3, 23 % obj").await;
    k9::assert_equal!(result, valuevec![Value::Integer(1), Value::Integer(3)]);
}

#[tokio::test]
async fn userdata_shl_binopside_via_vm() {
    use shingetsu::{userdata, BinOpSide, Value};
    use std::sync::Arc;

    struct Bits(i64);

    #[userdata]
    impl Bits {
        #[lua_metamethod(Shl)]
        fn shl_mm(&self, other: BinOpSide<i64>) -> i64 {
            other.impl_shl(self.0)
        }
    }

    let env = new_env();
    env.set_global("obj", Value::Userdata(Arc::new(Bits(1))));
    let result = run_with_env(env, "return obj << 4, 3 << obj").await;
    k9::assert_equal!(result, valuevec![Value::Integer(16), Value::Integer(6)]);
}

#[tokio::test]
async fn userdata_shr_binopside_via_vm() {
    use shingetsu::{userdata, BinOpSide, Value};
    use std::sync::Arc;

    struct Bits(i64);

    #[userdata]
    impl Bits {
        #[lua_metamethod(Shr)]
        fn shr_mm(&self, other: BinOpSide<i64>) -> i64 {
            other.impl_shr(self.0)
        }
    }

    let env = new_env();
    env.set_global("obj", Value::Userdata(Arc::new(Bits(16))));
    let result = run_with_env(env, "return obj >> 2, 128 >> obj").await;
    k9::assert_equal!(result, valuevec![Value::Integer(4), Value::Integer(0)]);
}

#[tokio::test]
async fn userdata_add_into_inner_via_vm() {
    use shingetsu::{userdata, BinOpSide, Value};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_metamethod(Add)]
        fn add_mm(&self, other: BinOpSide<i64>) -> i64 {
            self.0 + other.into_inner()
        }
    }

    let env = new_env();
    env.set_global("obj", Value::Userdata(Arc::new(Num(10))));
    let result = run_with_env(env, "return obj + 5, 5 + obj").await;
    k9::assert_equal!(result, valuevec![Value::Integer(15), Value::Integer(15)]);
}

#[tokio::test]
async fn userdata_add_convenience_via_vm() {
    use shingetsu::{userdata, BinOpSide, Value};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_metamethod(Add)]
        fn add_mm(&self, other: BinOpSide<i64>) -> i64 {
            other.add(self.0)
        }
    }

    let env = new_env();
    env.set_global("obj", Value::Userdata(Arc::new(Num(10))));
    let result = run_with_env(env, "return obj + 5, 5 + obj").await;
    k9::assert_equal!(result, valuevec![Value::Integer(15), Value::Integer(15)]);
}

#[tokio::test]
async fn userdata_concat_rhs_via_vm() {
    use shingetsu::{userdata, BinOpSide, Value};
    use std::sync::Arc;

    struct Tag(String);

    #[userdata]
    impl Tag {
        #[lua_metamethod(Concat)]
        fn concat_mm(&self, other: BinOpSide<String>) -> String {
            other.apply(self.0.clone(), |lhs, rhs| format!("{lhs}{rhs}"))
        }
    }

    let env = new_env();
    env.set_global("obj", Value::Userdata(Arc::new(Tag("world".into()))));
    let result = run_with_env(env, "return \"hello\" .. obj, obj .. \"!\"").await;
    k9::assert_equal!(
        result,
        valuevec![
            Value::String("helloworld".into()),
            Value::String("world!".into()),
        ]
    );
}

#[tokio::test]
async fn userdata_le_both_directions_via_vm() {
    use shingetsu::{userdata, BinOpSide, Value};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_metamethod(Le)]
        fn le_mm(&self, other: BinOpSide<i64>) -> bool {
            other.impl_le(self.0)
        }
    }

    let env = new_env();
    env.set_global("obj", Value::Userdata(Arc::new(Num(5))));
    let result = run_with_env(env, "return obj <= 5, obj <= 4, obj >= 5, obj >= 6").await;
    k9::assert_equal!(
        result,
        valuevec![
            Value::Boolean(true),
            Value::Boolean(false),
            Value::Boolean(true),
            Value::Boolean(false),
        ]
    );
}

#[tokio::test]
async fn userdata_binopside_with_f64_via_vm() {
    use shingetsu::{userdata, BinOpSide, Value};
    use std::sync::Arc;

    struct Num(f64);

    #[userdata]
    impl Num {
        #[lua_metamethod(Sub)]
        fn sub_mm(&self, other: BinOpSide<f64>) -> f64 {
            other.impl_sub(self.0)
        }
    }

    let env = new_env();
    env.set_global("obj", Value::Userdata(Arc::new(Num(2.5))));
    let result = run_with_env(env, "return obj - 1.0, 10.0 - obj").await;
    k9::assert_equal!(result, valuevec![Value::Float(1.5), Value::Float(7.5)]);
}

#[tokio::test]
async fn userdata_missing_metamethod_error() {
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    struct Empty;

    #[userdata]
    impl Empty {}

    let env = new_env();
    env.set_global("obj", Value::Userdata(Arc::new(Empty)));
    let err = run_err_with_env(env, "return obj + 1").await;
    k9::assert_equal!(
        err,
        "\
error: error in 'Empty:__add': metamethod '__add' not implemented for 'Empty'
 --> test.lua:1:8
  |
1 | return obj + 1
  |        ^^^^^^^ error in 'Empty:__add': metamethod '__add' not implemented for 'Empty'
stack traceback:
\ttest.lua:1: in main chunk"
    );
}
