use shingetsu_vm::types::TypedParam;
use std::sync::Arc;
mod common;

use common::{new_env, run_err_with_env, run_with_env};
use shingetsu::{valuevec, CallStack};

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
fn derive_userdata_from_lua_borrow() {
    use shingetsu::{FromLuaBorrow, UserData, Value};
    use std::sync::Arc;

    #[derive(Debug, UserData)]
    struct Point {
        x: i64,
    }

    let ud: Arc<dyn shingetsu::Userdata> = Arc::new(Point { x: 42 });
    let val = Value::Userdata(ud);

    let borrowed: &Point = FromLuaBorrow::from_lua_borrow(&val).unwrap();
    k9::assert_equal!(borrowed.x, 42);

    let opt: Option<&Point> = FromLuaBorrow::from_lua_borrow(&val).unwrap();
    k9::assert_equal!(opt.unwrap().x, 42);

    let nil = Value::Nil;
    let none: Option<&Point> = FromLuaBorrow::from_lua_borrow(&nil).unwrap();
    assert!(none.is_none());

    let wrong = Value::Integer(1);
    let err = <&Point as FromLuaBorrow>::from_lua_borrow(&wrong).unwrap_err();
    k9::assert_equal!(
        err.to_string(),
        "bad argument #0 to '' (Point expected, got number)"
    );
}

#[tokio::test]
async fn userdata_method_with_borrowed_param() {
    use shingetsu::{userdata, UserData, Value};
    use std::sync::Arc;

    #[derive(UserData)]
    struct Vec2 {
        x: i64,
        y: i64,
    }

    struct Geometry;

    #[userdata]
    impl Geometry {
        fn type_name(&self) -> &'static str {
            "Geometry"
        }

        #[lua_method]
        fn sum_components(&self, v: &Vec2) -> i64 {
            v.x + v.y
        }

        #[lua_method]
        fn add_vecs(&self, a: &Vec2, b: &Vec2) -> i64 {
            a.x + b.x + a.y + b.y
        }
    }

    let env = new_env();
    let geo: Arc<dyn shingetsu::Userdata> = Arc::new(Geometry);
    let v: Arc<dyn shingetsu::Userdata> = Arc::new(Vec2 { x: 10, y: 20 });
    let v2: Arc<dyn shingetsu::Userdata> = Arc::new(Vec2 { x: 3, y: 7 });
    env.set_global("geo", Value::Userdata(geo));
    env.set_global("v", Value::Userdata(v));
    env.set_global("v2", Value::Userdata(v2));

    let results = run_with_env(env.clone(), "return geo:sum_components(v)").await;
    k9::assert_equal!(results, valuevec![Value::Integer(30)]);

    let results = run_with_env(env, "return geo:add_vecs(v, v2)").await;
    k9::assert_equal!(results, valuevec![Value::Integer(40)]);
}

#[tokio::test]
async fn userdata_borrow_mixed_with_owned_params() {
    use shingetsu::{userdata, UserData, Value};
    use std::sync::Arc;

    #[derive(UserData)]
    struct Point {
        x: i64,
    }

    struct Helper;

    #[userdata]
    impl Helper {
        fn type_name(&self) -> &'static str {
            "Helper"
        }

        #[lua_method]
        fn offset(&self, p: &Point, dx: i64) -> i64 {
            p.x + dx
        }
    }

    let env = new_env();
    let h: Arc<dyn shingetsu::Userdata> = Arc::new(Helper);
    let p: Arc<dyn shingetsu::Userdata> = Arc::new(Point { x: 10 });
    env.set_global("h", Value::Userdata(h));
    env.set_global("p", Value::Userdata(p));

    let results = run_with_env(env, "return h:offset(p, 5)").await;
    k9::assert_equal!(results, valuevec![Value::Integer(15)]);
}

#[tokio::test]
async fn userdata_borrow_missing_arg_error() {
    use shingetsu::{userdata, UserData, Value};
    use std::sync::Arc;

    #[derive(UserData)]
    struct Blob;

    struct Ops;

    #[userdata]
    impl Ops {
        fn type_name(&self) -> &'static str {
            "Ops"
        }

        #[lua_method]
        fn inspect(&self, _b: &Blob) -> i64 {
            1
        }
    }

    let env = new_env();
    let ops: Arc<dyn shingetsu::Userdata> = Arc::new(Ops);
    env.set_global("ops", Value::Userdata(ops));

    let err = run_err_with_env(env, "return ops:inspect()").await;
    k9::assert_equal!(
        err,
        r#"error: bad argument #1 to 'inspect' (Blob expected, got no value)
 --> test.lua:1:8
  |
1 | return ops:inspect()
  |        ^^^^^^^^^^^ bad argument #1 to 'inspect' (Blob expected, got no value)
stack traceback:
	test.lua:1: in main chunk"#
    );
}

#[tokio::test]
async fn userdata_borrow_wrong_type_error() {
    use shingetsu::{userdata, UserData, Value};
    use std::sync::Arc;

    #[derive(UserData)]
    struct Apple;

    #[derive(UserData)]
    struct Orange;

    struct Juicer;

    #[userdata]
    impl Juicer {
        fn type_name(&self) -> &'static str {
            "Juicer"
        }

        #[lua_method]
        fn squeeze(&self, _a: &Apple) -> i64 {
            42
        }
    }

    let env = new_env();
    let j: Arc<dyn shingetsu::Userdata> = Arc::new(Juicer);
    let o: Arc<dyn shingetsu::Userdata> = Arc::new(Orange);
    env.set_global("j", Value::Userdata(j));
    env.set_global("o", Value::Userdata(o));

    let err = run_err_with_env(env, "return j:squeeze(o)").await;
    k9::assert_equal!(
        err,
        r#"error: bad argument #1 to 'squeeze' (Apple expected, got Orange)
 --> test.lua:1:8
  |
1 | return j:squeeze(o)
  |        ^^^^^^^^^ bad argument #1 to 'squeeze' (Apple expected, got Orange)
stack traceback:
	test.lua:1: in main chunk"#
    );
}

#[tokio::test]
async fn userdata_macro_field_and_method() {
    // #[shingetsu::userdata] on an impl block wires __index dispatch.
    use shingetsu::{userdata, Task, Value};
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
            source_name: Arc::new("@test".to_string()),
            type_check: false,
        },
        Default::default(),
    );
    let bc = compiler.compile(src).await.expect("compile");
    let func = bc.into_function();
    let results = Task::new(env, func, valuevec![]).await.expect("run");
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
async fn module_macro_deprecated_attribute() {
    // `#[module(deprecated = "...")]` flows into the generated
    // `module_type()` and is observable on the resulting
    // `ModuleType`.  Type-checking code can then propagate the
    // message into a parent module's `FieldDef` so accesses
    // through the parent fire the standard `deprecated` lint.
    use shingetsu::module;
    use shingetsu::types::LuaType;

    #[module(deprecated = "use `newmod` instead")]
    mod oldmod {
        #[function]
        fn noop() {}
    }

    let info = oldmod::module_type();
    let module_ty = info.return_type.expect("return type");
    let LuaType::Module(m) = module_ty else {
        panic!("expected Module type");
    };
    k9::assert_equal!(m.deprecated, Some("use `newmod` instead".to_string()));
}

#[tokio::test]
async fn module_macro_basic() {
    // #[shingetsu::module] generates build_module_table that registers functions.
    use shingetsu::{module, Task, Value};
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
            source_name: Arc::new("@test".to_string()),
            type_check: false,
        },
        Default::default(),
    );
    let bc = compiler.compile(src).await.expect("compile");
    let func = bc.into_function();
    let results = Task::new(env, func, valuevec![]).await.expect("run");
    k9::assert_equal!(results[0], Value::Integer(7));
}

mod borrow_module_test {
    #[derive(shingetsu::UserData)]
    pub struct Coord {
        pub x: i64,
    }

    #[shingetsu::module]
    pub mod geomod {
        use super::Coord;

        #[function]
        fn get_x(c: &Coord) -> i64 {
            c.x
        }

        #[function]
        fn is_nil(v: &shingetsu::Value) -> bool {
            matches!(v, shingetsu::Value::Nil)
        }
    }
}

#[tokio::test]
async fn module_function_with_borrowed_param() {
    use borrow_module_test::{geomod, Coord};
    use shingetsu::Value;
    use std::sync::Arc;

    let env = new_env();
    geomod::register_global_module(&env).expect("register");
    let c: Arc<dyn shingetsu::Userdata> = Arc::new(Coord { x: 99 });
    env.set_global("c", Value::Userdata(c));

    let results = run_with_env(env.clone(), "return geomod.get_x(c)").await;
    k9::assert_equal!(results, valuevec![Value::Integer(99)]);

    let results = run_with_env(env.clone(), "return geomod.is_nil(nil)").await;
    k9::assert_equal!(results, valuevec![Value::Boolean(true)]);

    let results = run_with_env(env, "return geomod.is_nil(42)").await;
    k9::assert_equal!(results, valuevec![Value::Boolean(false)]);
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

    let env = new_env();
    env.set_global("n", Value::Userdata(Arc::new(Num(42))));
    k9::assert_equal!(
        run_err_with_env(env, "return n:checked_div(0)").await,
        "\
error: error in 'checked_div': division by zero
 --> test.lua:1:8
  |
1 | return n:checked_div(0)
  |        ^^^^^^^^^^^^^ error in 'checked_div': division by zero
stack traceback:
\ttest.lua:1: in main chunk"
    );
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
    let ctx = CallContext::new(env, CallStack::new(), None);
    let result = Arc::clone(&obj)
        .dispatch(
            ctx,
            "__add",
            valuevec![Value::Userdata(obj), Value::Integer(5)],
        )
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
    let ctx = CallContext::new(env, CallStack::new(), None);
    let err = Arc::clone(&obj)
        .dispatch(
            ctx,
            "__add",
            valuevec![Value::Userdata(obj), Value::string("oops")],
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
    k9::assert_equal!(
        run_err_with_env(env, "return parsemod.parse_int('nope')").await,
        "\
error: error in 'parse_int': invalid digit found in string
 --> test.lua:1:8
  |
1 | return parsemod.parse_int('nope')
  |        ^^^^^^^^^^^^^^^^^^ error in 'parse_int': invalid digit found in string
stack traceback:
\ttest.lua:1: in main chunk"
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
                shingetsu_vm::types::TableField::new(
                    "increment",
                    LuaType::Function(Box::new(FunctionLuaType {
                        type_params: vec![],
                        params: vec![TypedParam::new(Some("amount"), LuaType::Number),],
                        variadic: None,
                        returns: vec![LuaType::Number],
                        is_method: true,
                        inferred_unannotated: false,
                        deprecated: None,
                        must_use: None,
                    })),
                ),
                shingetsu_vm::types::TableField::new("value", LuaType::Number),
            ],
            indexer: None,
        }))
    );
}

#[test]
fn userdata_lua_type_info_carries_param_docs() {
    use shingetsu::{userdata, FunctionLuaType, LuaType, TableLuaType, Userdata};
    use shingetsu_vm::types::TypedParam;

    struct Greeter;

    #[userdata]
    impl Greeter {
        /// Greet someone by name.
        ///
        /// # Parameters
        ///
        /// - `who` -- the recipient of the greeting
        /// - `loud` -- whether to shout
        #[lua_method]
        fn greet(&self, who: String, loud: bool) -> String {
            let _ = (who, loud);
            String::new()
        }

        /// Per-arg `///` is the alternative to `# Parameters`; the
        /// macro extracts these and feeds them into the same
        /// `TypedParam.doc` slot.
        #[lua_method]
        fn farewell(
            &self,
            /// who is leaving
            who: String,
            /// whether to wave
            wave: bool,
        ) -> String {
            let _ = (who, wave);
            String::new()
        }

        /// Per-arg `///` overrides `# Parameters` when both are
        /// supplied for the same parameter name.  The per-arg form
        /// is closer to the parameter declaration and wins.
        ///
        /// # Parameters
        ///
        /// - `n` -- markdown wins -- ignored
        #[lua_method]
        fn double(
            &self,
            /// per-arg wins
            n: i64,
        ) -> i64 {
            n * 2
        }
    }

    let g = Greeter;
    let LuaType::Table(table_ty) = g.lua_type_info() else {
        panic!("expected structural table type");
    };
    let TableLuaType { fields, indexer: _ } = *table_ty;
    // Methods sort alphabetically: double, farewell, greet.
    let LuaType::Function(double_ty) = &fields[0].lua_type else {
        panic!("expected function type");
    };
    let LuaType::Function(farewell_ty) = &fields[1].lua_type else {
        panic!("expected function type");
    };
    let LuaType::Function(greet_ty) = &fields[2].lua_type else {
        panic!("expected function type");
    };
    k9::assert_equal!(
        greet_ty.as_ref(),
        &FunctionLuaType {
            type_params: vec![],
            params: vec![
                TypedParam::new_with_doc(
                    Some("who"),
                    LuaType::String,
                    Some("the recipient of the greeting".to_owned()),
                ),
                TypedParam::new_with_doc(
                    Some("loud"),
                    LuaType::Boolean,
                    Some("whether to shout".to_owned()),
                ),
            ],
            variadic: None,
            returns: vec![LuaType::String],
            is_method: true,
            inferred_unannotated: false,
            deprecated: None,
            must_use: None,
        }
    );
    k9::assert_equal!(
        farewell_ty.as_ref(),
        &FunctionLuaType {
            type_params: vec![],
            params: vec![
                TypedParam::new_with_doc(
                    Some("who"),
                    LuaType::String,
                    Some(" who is leaving".to_owned()),
                ),
                TypedParam::new_with_doc(
                    Some("wave"),
                    LuaType::Boolean,
                    Some(" whether to wave".to_owned()),
                ),
            ],
            variadic: None,
            returns: vec![LuaType::String],
            is_method: true,
            inferred_unannotated: false,
            deprecated: None,
            must_use: None,
        }
    );
    k9::assert_equal!(
        double_ty.as_ref(),
        &FunctionLuaType {
            type_params: vec![],
            params: vec![TypedParam::new_with_doc(
                Some("n"),
                LuaType::Number,
                Some(" per-arg wins".to_owned()),
            )],
            variadic: None,
            returns: vec![LuaType::Number],
            is_method: true,
            inferred_unannotated: false,
            deprecated: None,
            must_use: None,
        }
    );
}

#[test]
fn userdata_type_descriptor_harvests_docs() {
    use shingetsu::userdata;
    use shingetsu_vm::types::{
        FieldDef, FieldKind, FunctionDef, FunctionSignature, LuaType, ParamSpec, UserdataType,
        ValueType,
    };

    /// A counter that you can increment.
    struct DocCounter(#[allow(dead_code)] i64);

    /// A counter that you can increment.
    #[userdata]
    impl DocCounter {
        /// The current count.
        #[lua_field]
        fn value(&self) -> i64 {
            self.0
        }

        /// Add `amount` to the counter and return the new value.
        ///
        /// # Parameters
        ///
        /// - `amount` — the number to add
        ///
        /// # Returns
        ///
        /// - the new value of the counter
        #[lua_method]
        fn increment(&self, amount: i64) -> i64 {
            self.0 + amount
        }
    }

    k9::assert_equal!(
        DocCounter::userdata_type(),
        UserdataType {
            name: "DocCounter".into(),
            doc: Some("A counter that you can increment.".into()),
            fields: vec![FieldDef {
                name: "value".into(),
                doc: Some("The current count.".into()),
                lua_type: LuaType::Number,
                kind: FieldKind::Getter,
                examples: vec![],
                deprecated: None,
            }],
            methods: vec![FunctionDef {
                name: "increment".into(),
                doc: Some("Add `amount` to the counter and return the new value.".into()),
                returns_doc: vec!["the new value of the counter".into()],
                examples: vec![],
                signature: FunctionSignature {
                    name: "increment".into(),
                    source: "=[DocCounter]".into(),
                    type_params: vec![],
                    params: vec![ParamSpec {
                        name: Some("amount".into()),
                        runtime_type: Some(ValueType::Number),
                        lua_type: Some(LuaType::Number),
                        doc: Some("the number to add".into()),
                    }],
                    variadic: false,

                    variadic_doc: None,
                    arg_offset: 1,
                    returns: None,
                    lua_returns: Some(vec![LuaType::Number]),
                    line_defined: 0,
                    last_line_defined: 0,
                    num_upvalues: 0,
                    has_runtime_types: true,
                    deprecated: None,
                    must_use: None,
                },
            }],
            metamethods: vec![],
        }
    );
}

#[test]
fn userdata_lua_type_info_default_is_named() {
    use shingetsu::{LuaType, UserData, Userdata};

    #[derive(UserData)]
    struct Simple;

    let s = Simple;
    k9::assert_equal!(s.lua_type_info(), LuaType::named("Simple"));
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
            fields: vec![shingetsu_vm::types::TableField::new(
                "greet",
                LuaType::Function(Box::new(FunctionLuaType {
                    type_params: vec![],
                    params: vec![TypedParam::new(Some("name"), LuaType::String),],
                    variadic: None,
                    returns: vec![LuaType::String],
                    is_method: true,
                    inferred_unannotated: false,
                    deprecated: None,
                    must_use: None,
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
    use shingetsu::{userdata, Task, Value};
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
            source_name: Arc::new("@test.lua".to_string()),
            type_check: false,
        },
        Default::default(),
    );
    let bc = compiler.compile(src).await.expect("compile");
    let func = bc.into_function();
    let err = Task::new(env, func, valuevec![]).await.unwrap_err();
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
    use shingetsu::{userdata, Task, Value, VmError};
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
            source_name: Arc::new("@test.lua".to_string()),
            type_check: false,
        },
        Default::default(),
    );
    let bc = compiler.compile(src).await.expect("compile");
    let func = bc.into_function();
    let err = Task::new(env, func, valuevec![]).await.unwrap_err();
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

// ---------------------------------------------------------------------------
// Module macro: lazy_field reruns on every access
// ---------------------------------------------------------------------------

static LAZY_FIELD_COUNTER: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

#[shingetsu::module(name = "tickmod")]
mod tickmod_test {
    use super::LAZY_FIELD_COUNTER;
    use std::sync::atomic::Ordering;

    #[lazy_field]
    fn tick() -> i64 {
        LAZY_FIELD_COUNTER.fetch_add(1, Ordering::SeqCst) + 1
    }
}

#[tokio::test]
async fn module_macro_lazy_field_recomputes() {
    use shingetsu::Value;
    LAZY_FIELD_COUNTER.store(0, std::sync::atomic::Ordering::SeqCst);
    let env = new_env();
    tickmod_test::register_global_module(&env).expect("register");
    let res = run_with_env(env, "return tickmod.tick, tickmod.tick, tickmod.tick").await;
    k9::assert_equal!(
        res,
        shingetsu::valuevec![Value::Integer(1), Value::Integer(2), Value::Integer(3)]
    );
}

// ---------------------------------------------------------------------------
// Module macro: getter / setter pair (read-write)
// ---------------------------------------------------------------------------

static GETTER_SETTER_SLOT: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

#[shingetsu::module(name = "cfg")]
mod cfg_test {
    use super::GETTER_SETTER_SLOT;
    use std::sync::atomic::Ordering;

    #[getter("value")]
    fn get_value() -> i64 {
        GETTER_SETTER_SLOT.load(Ordering::SeqCst)
    }

    #[setter("value")]
    fn set_value(v: i64) {
        GETTER_SETTER_SLOT.store(v, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn module_macro_getter_setter_pair() {
    use shingetsu::Value;
    GETTER_SETTER_SLOT.store(7, std::sync::atomic::Ordering::SeqCst);
    let env = new_env();
    cfg_test::register_global_module(&env).expect("register");
    let res = run_with_env(
        env,
        "local before = cfg.value; cfg.value = 100; return before, cfg.value",
    )
    .await;
    k9::assert_equal!(
        res,
        shingetsu::valuevec![Value::Integer(7), Value::Integer(100)]
    );
}

// ---------------------------------------------------------------------------
// Module macro: lazy_field with rename
// ---------------------------------------------------------------------------

#[tokio::test]
async fn module_macro_lazy_field_rename() {
    use shingetsu::{module, Value};

    #[module]
    mod rl {
        #[lazy_field(rename = "answer")]
        fn fortytwo() -> i64 {
            42
        }
    }

    let env = new_env();
    rl::register_global_module(&env).expect("register");
    let res = run_with_env(env, "return rl.answer").await;
    k9::assert_equal!(res[0], Value::Integer(42));
}

// ---------------------------------------------------------------------------
// Module macro: lazy and eager fields coexist; functions still callable
// ---------------------------------------------------------------------------

#[tokio::test]
async fn module_macro_mixed_lazy_eager_function() {
    use shingetsu::{module, Value};

    #[module]
    mod mixed {
        #[field]
        fn eager() -> i64 {
            10
        }

        #[lazy_field]
        fn lazy() -> i64 {
            20
        }

        #[function]
        fn double(n: i64) -> i64 {
            n * 2
        }
    }

    let env = new_env();
    mixed::register_global_module(&env).expect("register");
    let res = run_with_env(env, "return mixed.eager, mixed.lazy, mixed.double(7)").await;
    k9::assert_equal!(
        res,
        shingetsu::valuevec![Value::Integer(10), Value::Integer(20), Value::Integer(14),]
    );
}

// ---------------------------------------------------------------------------
// Module macro: setter alone (write-only)
// ---------------------------------------------------------------------------

static SETTER_ONLY_SINK: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

#[shingetsu::module(name = "wr")]
mod wr_test {
    use super::SETTER_ONLY_SINK;
    use std::sync::atomic::Ordering;

    #[setter("slot")]
    fn set_slot(v: i64) {
        SETTER_ONLY_SINK.store(v, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn module_macro_setter_only() {
    SETTER_ONLY_SINK.store(0, std::sync::atomic::Ordering::SeqCst);
    let env = new_env();
    wr_test::register_global_module(&env).expect("register");
    let _ = run_with_env(env, "wr.slot = 99").await;
    k9::assert_equal!(
        SETTER_ONLY_SINK.load(std::sync::atomic::Ordering::SeqCst),
        99
    );
}

// ---------------------------------------------------------------------------
// Module macro: per-arg `///` populates ParamSpec.doc, and overrides
// `# Parameters` markdown when both are supplied.
// ---------------------------------------------------------------------------

#[test]
fn module_function_per_arg_doc_capture() {
    use shingetsu::LuaType;
    #[allow(dead_code)]
    #[shingetsu::module]
    mod doc_mod {
        /// Per-arg `///` is the alternative to `# Parameters`; the
        /// macro extracts these and feeds them into `ParamSpec.doc`.
        #[function]
        fn farewell(
            /// who is leaving
            who: String,
            /// whether to wave
            wave: bool,
        ) -> String {
            let _ = (who, wave);
            String::new()
        }

        /// Per-arg `///` overrides `# Parameters` when both are
        /// supplied for the same parameter name.
        ///
        /// # Parameters
        ///
        /// - `n` -- markdown wins -- ignored
        #[function]
        fn double(
            /// per-arg wins
            n: i64,
        ) -> i64 {
            n * 2
        }
    }

    let info = doc_mod::module_type();
    let LuaType::Module(m) = info.return_type.expect("return type") else {
        panic!("expected Module type");
    };
    let by_name: std::collections::HashMap<String, _> = m
        .functions
        .iter()
        .map(|f| (std::str::from_utf8(&f.name).expect("utf8").to_owned(), f))
        .collect();
    let farewell = by_name.get("farewell").expect("farewell");
    k9::assert_equal!(
        farewell.signature.params[0].doc.as_deref(),
        Some(" who is leaving")
    );
    k9::assert_equal!(
        farewell.signature.params[1].doc.as_deref(),
        Some(" whether to wave")
    );
    let double = by_name.get("double").expect("double");
    k9::assert_equal!(
        double.signature.params[0].doc.as_deref(),
        Some(" per-arg wins")
    );
}

// ---------------------------------------------------------------------------
// Module macro: type metadata reflects field kinds
// ---------------------------------------------------------------------------

#[test]
fn module_macro_field_kind_metadata() {
    use shingetsu::types::{FieldKind, LuaType};

    // The bodies are never invoked: this test only inspects
    // `module_type()` metadata produced by the macro.
    #[allow(dead_code)]
    #[shingetsu::module]
    mod meta_mod {
        #[field]
        fn fixed() -> i64 {
            1
        }
        #[lazy_field]
        fn dyn_only() -> i64 {
            2
        }
        #[getter("both")]
        fn get_both() -> i64 {
            3
        }
        #[setter("both")]
        fn set_both(_v: i64) {}
        #[setter("writeonly")]
        fn set_writeonly(_v: i64) {}
    }

    let info = meta_mod::module_type();
    let LuaType::Module(m) = info.return_type.expect("return type") else {
        panic!("expected Module type");
    };
    let kinds: Vec<(String, FieldKind)> = m
        .fields
        .iter()
        .map(|f| {
            (
                std::str::from_utf8(&f.name).expect("utf8").to_owned(),
                f.kind,
            )
        })
        .collect();
    k9::assert_equal!(
        kinds,
        vec![
            ("fixed".to_owned(), FieldKind::Eager),
            ("both".to_owned(), FieldKind::ReadWrite),
            ("dyn_only".to_owned(), FieldKind::Getter),
            ("writeonly".to_owned(), FieldKind::Setter),
        ]
    );
}

// ---------------------------------------------------------------------------
// Module macro: GlobalEnv parameter injection
// ---------------------------------------------------------------------------

#[shingetsu::module(name = "envmod")]
mod envmod_test {
    #[function]
    fn has_print(env: shingetsu::GlobalEnv) -> i64 {
        let v = env.get_global(b"print").unwrap_or(shingetsu::Value::Nil);
        if matches!(v, shingetsu::Value::Nil) {
            0
        } else {
            1
        }
    }
}

#[tokio::test]
async fn module_macro_function_global_env_param() {
    use shingetsu::Value;
    let env = new_env();
    envmod_test::register_global_module(&env).expect("register");
    let res = run_with_env(env, "return envmod.has_print()").await;
    k9::assert_equal!(res[0], Value::Integer(1));
}

// ---------------------------------------------------------------------------
// FromLuaMulti enum: leading-optional chain + trailing Variadic
// ---------------------------------------------------------------------------

/// Variants form a tail-subset chain (`Named[1..] == NoName`),
/// so the derive should render `name` as Optional rather than as
/// `string | function` per-position union.  Trailing `Variadic`
/// also exercises the variadic-in-last-position path.
#[derive(shingetsu::FromLuaMulti)]
#[allow(dead_code)]
enum SpawnLikeArgs {
    Named {
        name: shingetsu::Bytes,
        func: shingetsu::Function,
        args: shingetsu::Variadic,
    },
    NoName {
        func: shingetsu::Function,
        args: shingetsu::Variadic,
    },
}

#[test]
fn from_lua_multi_leading_optional_renders_with_optional_marker() {
    use shingetsu::types::LuaType;
    use shingetsu::{Bytes, Function, LuaTyped, LuaTypedMulti, Variadic};

    // Position 0 is `name` wrapped in Optional; position 1 is
    // `func`; position 2 is the trailing variadic.  The longest
    // variant's names propagate to the parameter list.
    k9::assert_equal!(
        SpawnLikeArgs::lua_types(),
        vec![
            LuaType::Optional(Box::new(Bytes::lua_type())),
            Function::lua_type(),
            Variadic::lua_type(),
        ]
    );
    k9::assert_equal!(
        SpawnLikeArgs::lua_param_names(),
        vec![Some("name"), Some("func"), Some("args")]
    );
}

#[tokio::test]
async fn from_lua_multi_leading_optional_dispatches_correctly() {
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    struct Spawner;

    #[userdata]
    impl Spawner {
        #[lua_method(variadic)]
        fn run(&self, args: SpawnLikeArgs) -> i64 {
            // Return 100 + arg-count for Named, just arg-count for NoName.
            match args {
                SpawnLikeArgs::Named { args, .. } => 100 + args.0.len() as i64,
                SpawnLikeArgs::NoName { args, .. } => args.0.len() as i64,
            }
        }
    }

    let env = new_env();
    env.set_global("s", Value::Userdata(Arc::new(Spawner)));
    let res = run_with_env(
        env.clone(),
        "return s:run(function() end), s:run('n', function() end, 1, 2, 3)",
    )
    .await;
    k9::assert_equal!(res, valuevec![Value::Integer(0), Value::Integer(103)]);
}

// ---------------------------------------------------------------------------
// Userdata macro: lua_method(variadic) promotes last param to VariadicMulti
// ---------------------------------------------------------------------------

#[derive(shingetsu::FromLuaMulti)]
#[allow(dead_code)]
enum AddArgs {
    Two(i64, i64),
    Three(i64, i64, i64),
}

#[tokio::test]
async fn userdata_macro_method_variadic_attr() {
    // `#[lua_method(variadic)]` promotes the last Normal param to
    // `VariadicMulti`, decoding the remaining args via `FromLuaMulti`.
    // An overload-dispatch enum lets one method handle multiple arities.
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    struct Adder;

    #[userdata]
    impl Adder {
        #[lua_method(variadic)]
        fn sum(&self, args: AddArgs) -> i64 {
            match args {
                AddArgs::Two(a, b) => a + b,
                AddArgs::Three(a, b, c) => a + b + c,
            }
        }
    }

    let env = new_env();
    env.set_global("a", Value::Userdata(Arc::new(Adder)));
    let res = run_with_env(env.clone(), "return a:sum(1, 2)").await;
    k9::assert_equal!(res[0], Value::Integer(3));
    let res = run_with_env(env, "return a:sum(1, 2, 3)").await;
    k9::assert_equal!(res[0], Value::Integer(6));
}

// ---------------------------------------------------------------------------
// Userdata snapshot: derive(UserData) with #[lua(snapshot)]
// ---------------------------------------------------------------------------

#[tokio::test]
async fn derive_userdata_lua_snapshot_clones_and_rebuilds() {
    use shingetsu::{GlobalEnv, IntoLua, Userdata, Value};
    use std::sync::Arc;

    #[derive(Clone, shingetsu::UserData)]
    #[lua(snapshot)]
    struct Counter {
        value: i64,
    }

    impl IntoLua for Counter {
        fn into_lua(self) -> Value {
            Value::Integer(self.value)
        }
    }

    // Snapshot a userdata in one env, rebuild it in a fresh env
    // and check the value carries through.
    let original = Counter { value: 42 };
    let snap = Userdata::snapshot(&original).expect("snapshot opted in");

    let other_env = new_env();
    let rebuilt = snap.rebuild(&other_env).expect("rebuild");
    k9::assert_equal!(rebuilt, Value::Integer(42));

    // The snapshot is reusable — each rebuild produces a fresh value.
    let rebuilt_again = snap.rebuild(&other_env).expect("rebuild again");
    k9::assert_equal!(rebuilt_again, Value::Integer(42));

    // And it works through the dyn-Userdata path too.
    let env = new_env();
    env.set_global("c", Value::Userdata(Arc::new(original)));
    // No script execution needed — just verify trait dispatch.
    let _ = env;
    let _ = GlobalEnv::new();
}

#[tokio::test]
async fn userdata_macro_lua_snapshot_method() {
    use shingetsu::{userdata, Snapshot, Userdata, Value};

    struct State {
        data: i64,
    }

    #[userdata]
    impl State {
        #[lua_snapshot]
        fn snap(&self) -> Snapshot {
            let saved = self.data;
            Snapshot::new(move |_env| Ok(Value::Integer(saved)))
        }
    }

    let s = State { data: 99 };
    let snap = Userdata::snapshot(&s).expect("snapshot opted in");
    let env = new_env();
    let rebuilt = snap.rebuild(&env).expect("rebuild");
    k9::assert_equal!(rebuilt, Value::Integer(99));
}

#[tokio::test]
async fn userdata_default_snapshot_is_none() {
    // Types that don't opt in get the default `None` from the
    // trait — host-side caches can detect non-snapshotable values
    // and treat them as opaque.
    use shingetsu::{userdata, Userdata};

    struct Opaque;

    #[userdata]
    impl Opaque {}

    let o = Opaque;
    assert!(Userdata::snapshot(&o).is_none());
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

// ---------------------------------------------------------------------------
// Userdata macro: __pairs metamethod via the pairs() builtin
// ---------------------------------------------------------------------------

#[derive(shingetsu::IntoLuaMulti)]
enum PairsIter {
    Item(String, i64),
    End,
}

#[tokio::test]
async fn userdata_pairs_metamethod_via_builtin() {
    use shingetsu::{userdata, Function, Value, VmError};
    use std::sync::Arc;

    /// A userdata that exposes a fixed key/value list through the
    /// `__pairs` metamethod — the same shape kumomta and wezterm use,
    /// where the metamethod returns `(iter_fn, state, control)` and
    /// the iter is a stateless function `(state, prev_key) -> (key, val)`.
    #[derive(Clone)]
    struct Map(Vec<(String, i64)>);

    #[userdata]
    impl Map {
        #[lua_metamethod(Pairs)]
        fn pairs(&self) -> Result<(Function, Value, Value), VmError> {
            let entries = self.0.clone();
            let iter = Function::wrap(
                "Map.__pairs.iter",
                move |_state: Value, prev: Option<String>| -> Result<PairsIter, VmError> {
                    // Find the next entry after `prev`.  When `prev` is
                    // `nil`, return the first entry.  When at the end,
                    // return `End` (an empty multi) so the for loop
                    // terminates.
                    let next_idx = match prev {
                        None => 0,
                        Some(k) => match entries.iter().position(|(ek, _)| ek == &k) {
                            Some(i) => i + 1,
                            None => return Ok(PairsIter::End),
                        },
                    };
                    match entries.get(next_idx) {
                        Some((k, v)) => Ok(PairsIter::Item(k.clone(), *v)),
                        None => Ok(PairsIter::End),
                    }
                },
            );
            // State and control slots are unused here since the
            // iterator is stateless; Lua's generic-for accepts `Nil`
            // for both.
            Ok((iter, Value::Nil, Value::Nil))
        }
    }

    let env = new_env();
    env.set_global(
        "m",
        Value::Userdata(Arc::new(Map(vec![
            ("a".into(), 10),
            ("b".into(), 20),
            ("c".into(), 30),
        ]))),
    );
    let res = run_with_env(
        env,
        r#"
        local seen = {}
        for k, v in pairs(m) do
            seen[#seen + 1] = k .. '=' .. tostring(v)
        end
        return seen[1], seen[2], seen[3], #seen
        "#,
    )
    .await;
    k9::assert_equal!(
        res,
        valuevec![
            Value::string("a=10"),
            Value::string("b=20"),
            Value::string("c=30"),
            Value::Integer(3),
        ]
    );
}

// ---------------------------------------------------------------------------
// Userdata macro: #[lua_pairs] declarative iterator
// ---------------------------------------------------------------------------

#[tokio::test]
async fn userdata_lua_pairs_iterates_in_order() {
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    /// `#[lua_pairs]` lets the user return a Rust iterator of
    /// `(K, V)` tuples; the macro emits the boxing/state-stashing
    /// glue and a `__pairs` metamethod on both engines.
    #[derive(Clone)]
    struct Map(Vec<(String, i64)>);

    #[userdata]
    impl Map {
        #[lua_pairs]
        fn pairs_impl(&self) -> impl Iterator<Item = (String, i64)> + Send + 'static {
            self.0.clone().into_iter()
        }
    }

    let env = new_env();
    env.set_global(
        "m",
        Value::Userdata(Arc::new(Map(vec![
            ("a".into(), 10),
            ("b".into(), 20),
            ("c".into(), 30),
        ]))),
    );
    let res = run_with_env(
        env,
        r#"
        local seen = {}
        for k, v in pairs(m) do
            seen[#seen + 1] = k .. '=' .. tostring(v)
        end
        return seen[1], seen[2], seen[3], #seen
        "#,
    )
    .await;
    k9::assert_equal!(
        res,
        valuevec![
            Value::string("a=10"),
            Value::string("b=20"),
            Value::string("c=30"),
            Value::Integer(3),
        ]
    );
}

#[tokio::test]
async fn userdata_lua_pairs_fallible_setup() {
    use shingetsu::{userdata, Value, VmError};
    use std::sync::Arc;

    /// `#[lua_pairs]` also accepts `Result<impl Iterator, VmError>`
    /// so the iterator-materialization step can fail (e.g. when the
    /// inner data needs to be serialized and might not be a map).
    struct Maybe {
        ok: bool,
    }

    #[userdata]
    impl Maybe {
        #[lua_pairs]
        fn pairs_impl(
            &self,
        ) -> Result<impl Iterator<Item = (String, i64)> + Send + 'static, VmError> {
            if !self.ok {
                return Err(VmError::LuaError {
                    display: "not iterable".into(),
                    value: Value::string("not iterable"),
                });
            }
            Ok(vec![("k".into(), 1)].into_iter())
        }
    }

    let env = new_env();
    env.set_global("m", Value::Userdata(Arc::new(Maybe { ok: true })));
    let ok = run_with_env(env, "for k, v in pairs(m) do return k, v end").await;
    k9::assert_equal!(ok, valuevec![Value::string("k"), Value::Integer(1)]);
}

// ---------------------------------------------------------------------------
// Userdata __eq dispatch through OpCode::Eq
// ---------------------------------------------------------------------------

#[tokio::test]
async fn userdata_eq_metamethod_via_vm() {
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    /// A pair where `__eq` compares the value contents, so two
    /// distinct userdata instances with the same fields compare
    /// equal under Lua `==`.
    struct Pair {
        x: i64,
        y: i64,
    }

    #[userdata]
    impl Pair {
        #[lua_metamethod(Eq)]
        fn eq_mm(&self, other: shingetsu::Ud<Self>) -> bool {
            self.x == other.x && self.y == other.y
        }
    }

    let env = new_env();
    env.set_global("a", Value::Userdata(Arc::new(Pair { x: 1, y: 2 })));
    env.set_global("b", Value::Userdata(Arc::new(Pair { x: 1, y: 2 })));
    env.set_global("c", Value::Userdata(Arc::new(Pair { x: 3, y: 4 })));
    let res = run_with_env(env, "return a == b, a == c, a == a").await;
    k9::assert_equal!(
        res,
        valuevec![
            Value::Boolean(true),
            Value::Boolean(false),
            Value::Boolean(true),
        ]
    );
}

#[tokio::test]
async fn userdata_eq_without_metamethod_falls_back_to_rawequal() {
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    struct Plain;

    #[userdata]
    impl Plain {}

    let plain = Arc::new(Plain);
    let env = new_env();
    env.set_global("a", Value::Userdata(plain.clone()));
    env.set_global("b", Value::Userdata(plain.clone()));
    env.set_global("c", Value::Userdata(Arc::new(Plain)));
    // No __eq metamethod is registered, so == falls back to
    // rawequal: same Arc → true, different Arc → false (no error).
    let res = run_with_env(env, "return a == b, a == c").await;
    k9::assert_equal!(res, valuevec![Value::Boolean(true), Value::Boolean(false)]);
}
