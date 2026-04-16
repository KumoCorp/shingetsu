mod common;

use common::{new_env, run_with_env};

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

#[test]
fn userdata_macro_field_and_method() {
    // #[shingetsu::userdata] on an impl block wires __index dispatch.
    use shingetsu::{userdata, Function, Task, Value};
    use shingetsu_compiler::{compile, CompileOptions};
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
    let opts = CompileOptions {
        debug_info: false,
        source_name: "test".into(),
    };
    let bc = compile(src, &opts).expect("compile");
    let func = Function::lua(bc.top_level, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("rt");
    let results = rt.block_on(Task::new(env, func, vec![])).expect("run");
    k9::assert_equal!(results[0], Value::Integer(42));
}

#[test]
fn typeof_on_userdata_returns_host_type_name() {
    // typeof() surfaces the Userdata::type_name() value for userdata
    // values, whereas type() always returns "userdata".
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    struct Counter(i64);

    #[userdata]
    impl Counter {
        fn type_name(&self) -> &'static str {
            "Counter"
        }
    }

    let env = new_env();
    env.set_global("c", Value::Userdata(Arc::new(Counter(1))));
    let res = run_with_env(env, "return type(c), typeof(c)");
    k9::assert_equal!(
        res,
        vec![Value::string("userdata"), Value::string("Counter")]
    );
}

#[test]
fn module_macro_basic() {
    // #[shingetsu::module] generates build_module_table that registers functions.
    use shingetsu::{module, Function, Task, Value};
    use shingetsu_compiler::{compile, CompileOptions};

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
    let opts = CompileOptions {
        debug_info: false,
        source_name: "test".into(),
    };
    let bc = compile(src, &opts).expect("compile");
    let func = Function::lua(bc.top_level, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("rt");
    let results = rt.block_on(Task::new(env, func, vec![])).expect("run");
    k9::assert_equal!(results[0], Value::Integer(7));
}

// ---------------------------------------------------------------------------
// Userdata macro: field getter with rename
// ---------------------------------------------------------------------------

#[test]
fn userdata_macro_field_rename() {
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
    let res = run_with_env(env, "return pt.x + pt.y");
    k9::assert_equal!(res[0], Value::Integer(10));
}

// ---------------------------------------------------------------------------
// Userdata macro: field setter via set_ prefix
// ---------------------------------------------------------------------------

#[test]
fn userdata_macro_field_setter() {
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
    let res = run_with_env(env, "c.value = 99; return c.value");
    k9::assert_equal!(res[0], Value::Integer(99));
}

#[test]
fn validate_args_field_setter_rejects_wrong_type() {
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
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Boolean(false),
            Value::string(
                "bad value in assignment to 'Counter.value' (integer expected, got string)"
            ),
        ]
    );
}

// ---------------------------------------------------------------------------
// Userdata macro: method with &self receiver and a parameter
// ---------------------------------------------------------------------------

#[test]
fn userdata_macro_method_ref_self() {
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
    let res = run_with_env(env, "return n:multiply(6)");
    k9::assert_equal!(res[0], Value::Integer(42));
}

// ---------------------------------------------------------------------------
// Userdata macro: method with Arc<Self> receiver
// ---------------------------------------------------------------------------

#[test]
fn userdata_macro_method_arc_self() {
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
    let res = run_with_env(env, "return n:doubled()");
    k9::assert_equal!(res[0], Value::Integer(42));
}

// ---------------------------------------------------------------------------
// Userdata macro: method returning Result
// ---------------------------------------------------------------------------

#[test]
fn userdata_macro_method_result_ok() {
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
    let res = run_with_env(env, "return n:checked_div(6)");
    k9::assert_equal!(res[0], Value::Integer(7));
}

#[test]
fn userdata_macro_method_result_err() {
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
    use shingetsu_compiler::{compile, CompileOptions};

    let env = new_env();
    env.set_global("n", Value::Userdata(Arc::new(Num(42))));
    let opts = CompileOptions {
        debug_info: false,
        source_name: "test".into(),
    };
    let bc = compile("return n:checked_div(0)", &opts).expect("compile");
    let func = Function::lua(bc.top_level, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("rt");
    let err = rt.block_on(Task::new(env, func, vec![])).unwrap_err();
    k9::assert_equal!(err.to_string(), "error in 'checked_div': division by zero");
}

// ---------------------------------------------------------------------------
// Userdata macro: method with CallContext parameter
// ---------------------------------------------------------------------------

#[test]
fn userdata_macro_method_callcontext() {
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
    let res = run_with_env(env, "return d:run(21)");
    k9::assert_equal!(res[0], Value::Integer(42));
}

// ---------------------------------------------------------------------------
// Userdata macro: method with Variadic parameter
// ---------------------------------------------------------------------------

#[test]
fn userdata_macro_method_variadic() {
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
    let res = run_with_env(env, "return s:sum(1, 2, 3, 4)");
    k9::assert_equal!(res[0], Value::Integer(10));
}

// ---------------------------------------------------------------------------
// Userdata macro: __tostring metamethod via tostring() builtin
// ---------------------------------------------------------------------------

#[test]
fn userdata_macro_metamethod_tostring() {
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
    let res = run_with_env(env, "return tostring(obj)");
    k9::assert_equal!(res[0], Value::string("hello"));
}

// ---------------------------------------------------------------------------
// Userdata macro: binary metamethod dispatched directly
// ---------------------------------------------------------------------------

#[test]
fn userdata_macro_metamethod_binary_dispatch() {
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
    let rt = tokio::runtime::Runtime::new().expect("rt");
    let result = rt
        .block_on(Arc::clone(&obj).dispatch(
            ctx,
            "__add",
            vec![Value::Userdata(obj), Value::Integer(5)],
        ))
        .expect("dispatch");
    k9::assert_equal!(result[0], Value::Integer(15));
}

#[test]
fn validate_args_metamethod_rejects_wrong_type() {
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
    let rt = tokio::runtime::Runtime::new().expect("rt");
    let err = rt
        .block_on(Arc::clone(&obj).dispatch(
            ctx,
            "__add",
            vec![Value::Userdata(obj), Value::string("oops")],
        ))
        .unwrap_err();
    k9::assert_equal!(
        err.to_string(),
        "bad argument #1 to 'Num:__add' (integer expected, got string)"
    );
}

// ---------------------------------------------------------------------------
// Module macro: function returning Result (Ok path)
// ---------------------------------------------------------------------------

#[test]
fn module_macro_result_return() {
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
    let res = run_with_env(env, "return mathmod.checked_sqrt(4.0)");
    k9::assert_equal!(res[0], Value::Float(2.0));
}

// ---------------------------------------------------------------------------
// Module macro: async function
// ---------------------------------------------------------------------------

#[test]
fn module_macro_async_fn() {
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
    let res = run_with_env(env, "return asyncmod.async_double(21)");
    k9::assert_equal!(res[0], Value::Integer(42));
}

// ---------------------------------------------------------------------------
// Module macro: function with CallContext parameter
// ---------------------------------------------------------------------------

#[test]
fn module_macro_callcontext() {
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
    let res = run_with_env(env, "return ctxmod.passthrough(99)");
    k9::assert_equal!(res[0], Value::Integer(99));
}

// ---------------------------------------------------------------------------
// Module macro: function with Variadic parameter
// ---------------------------------------------------------------------------

#[test]
fn module_macro_variadic() {
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
    let res = run_with_env(env, "return varmod.sum_all(10, 20, 12)");
    k9::assert_equal!(res[0], Value::Integer(42));
}

// ---------------------------------------------------------------------------
// Module macro: eager field
// ---------------------------------------------------------------------------

#[test]
fn module_macro_eager_field() {
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
    let res = run_with_env(env, "return constmod.magic");
    k9::assert_equal!(res[0], Value::Integer(42));
}

// ---------------------------------------------------------------------------
// Module macro: function rename
// ---------------------------------------------------------------------------

#[test]
fn module_macro_function_rename() {
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
    let res = run_with_env(env, "return renmod.doThing(5)");
    k9::assert_equal!(res[0], Value::Integer(6));
}

// ---------------------------------------------------------------------------
// Module macro: module name option overrides global key
// ---------------------------------------------------------------------------

#[test]
fn module_macro_name_option() {
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
    let res = run_with_env(env, "return myMod.hello()");
    k9::assert_equal!(res[0], Value::Integer(1));
}

// ---------------------------------------------------------------------------
// Userdata macro: get_ prefix is stripped automatically for field names
// ---------------------------------------------------------------------------

#[test]
fn userdata_macro_field_get_prefix() {
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
    let res = run_with_env(env, "return r.width * r.height");
    k9::assert_equal!(res[0], Value::Integer(24));
}

// ---------------------------------------------------------------------------
// Userdata macro: set_ prefix extraction and setter dispatch
// ---------------------------------------------------------------------------

#[test]
fn userdata_macro_field_set_prefix() {
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
    let res = run_with_env(env, "b.side = 5; return b.side");
    k9::assert_equal!(res[0], Value::Integer(5));
}

// ---------------------------------------------------------------------------
// Result<T, E> where E: Into<VmError> — custom error type conversion
// ---------------------------------------------------------------------------

#[test]
fn module_macro_result_custom_error() {
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
    let res = run_with_env(env.clone(), "return parsemod.parse_int('42')");
    k9::assert_equal!(res[0], Value::Integer(42));

    // Err path: non-integer string surfaces as VmError.
    use shingetsu::{Function, Task};
    use shingetsu_compiler::{compile, CompileOptions};
    let opts = CompileOptions {
        debug_info: false,
        source_name: "test".into(),
    };
    let bc = compile("return parsemod.parse_int('nope')", &opts).expect("compile");
    let func = Function::lua(bc.top_level, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("rt");
    let err = rt.block_on(Task::new(env, func, vec![])).unwrap_err();
    k9::assert_equal!(
        err.to_string(),
        "error in 'parse_int': invalid digit found in string"
    );
}

// ---------------------------------------------------------------------------
// Module macro: `this` table parameter (colon-call passes module table)
// ---------------------------------------------------------------------------

#[test]
fn module_macro_this_param() {
    // When a module function is called with `:` syntax, Lua passes the module
    // table itself as the first argument.  Declaring `this: Table` captures it.
    use shingetsu::{module, Value};

    #[module(name = "tmod")]
    mod tmod_impl {
        use shingetsu::bytes::Bytes;
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
    let res = run_with_env(env, "return tmod:read_version()");
    k9::assert_equal!(res[0], Value::Integer(99));
}

// ---------------------------------------------------------------------------
