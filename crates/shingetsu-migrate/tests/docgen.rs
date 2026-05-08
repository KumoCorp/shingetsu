//! Confirms shingetsu's existing docgen pipeline picks up modules
//! and userdata types registered via the migration facade's
//! `#[shingetsu_migrate::module]` and `#[shingetsu_migrate::userdata]`
//! attribute macros.  The shingetsu side of those macros delegates
//! to the same registration calls (`register_preload_typed`,
//! `register_userdata_type`) that `extract` walks, so this test is
//! a regression guard rather than a behavior demonstration: if a
//! future change to the facade re-emit path ever drops one of those
//! calls, this test catches it before docgen output silently goes
//! empty for migrating hosts.

#![cfg(all(feature = "shingetsu-backend", feature = "mlua-backend"))]

use shingetsu_docgen::extract;
use shingetsu_migrate::shingetsu::GlobalEnv;

/// A representative module: an eager field, a sync function, and
/// rustdoc on each so docgen has summaries to capture.
#[shingetsu_migrate::module(name = "demo")]
mod demo {
    /// Static version string exposed as `demo.version`.
    #[field]
    fn version() -> String {
        "1.0".to_owned()
    }

    /// Add two integers and return the sum.
    #[function]
    fn add(a: i64, b: i64) -> i64 {
        a + b
    }
}

/// A representative userdata: one method and one metamethod, each
/// with rustdoc.
struct Counter(i64);

#[shingetsu_migrate::userdata]
impl Counter {
    /// Read the current count.
    #[lua_method]
    fn get(&self) -> i64 {
        self.0
    }

    /// `tostring(counter)` formats as `Counter(N)`.
    #[lua_metamethod(ToString)]
    fn ts(&self) -> String {
        format!("Counter({})", self.0)
    }
}

#[tokio::test]
async fn docgen_extracts_facade_decorated_module() {
    let env = GlobalEnv::new();
    // `register_preload` (not `register_global_module`) is the
    // entry point docgen walks; the host calls it for every module
    // they want surfaced in reference docs.
    demo::register_preload(&env);

    let model = extract(&env);
    let demo_mod = model
        .modules
        .iter()
        .find(|m| m.name == "demo")
        .expect("demo module present in DocModel");

    let field_names: Vec<String> = demo_mod.fields.iter().map(|f| f.name.clone()).collect();
    let fn_names: Vec<String> = demo_mod.functions.iter().map(|f| f.name.clone()).collect();
    k9::assert_equal!(field_names, vec!["version".to_owned()]);
    k9::assert_equal!(fn_names, vec!["add".to_owned()]);
}

#[tokio::test]
async fn docgen_extracts_facade_decorated_userdata() {
    let env = GlobalEnv::new();
    env.register_userdata_type(Counter::userdata_type());

    let model = extract(&env);
    let counter = model
        .userdata_types
        .iter()
        .find(|u| u.name == "Counter")
        .expect("Counter userdata type present in DocModel");

    let method_names: Vec<String> = counter.methods.iter().map(|m| m.name.clone()).collect();
    let metamethod_names: Vec<String> = counter
        .metamethods
        .iter()
        .map(|m| m.method.clone())
        .collect();
    k9::assert_equal!(method_names, vec!["get".to_owned()]);
    k9::assert_equal!(metamethod_names, vec!["__tostring".to_owned()]);
}

#[tokio::test]
async fn docgen_captures_rustdoc_summaries() {
    let env = GlobalEnv::new();
    demo::register_preload(&env);
    env.register_userdata_type(Counter::userdata_type());

    let model = extract(&env);

    let demo_mod = model
        .modules
        .iter()
        .find(|m| m.name == "demo")
        .expect("demo module");
    let version_field = demo_mod
        .fields
        .iter()
        .find(|f| f.name == "version")
        .expect("version field");
    k9::assert_equal!(
        version_field.doc,
        Some("Static version string exposed as `demo.version`.".to_owned())
    );
    let add_fn = demo_mod
        .functions
        .iter()
        .find(|f| f.name == "add")
        .expect("add function");
    k9::assert_equal!(
        add_fn.doc,
        Some("Add two integers and return the sum.".to_owned())
    );

    let counter = model
        .userdata_types
        .iter()
        .find(|u| u.name == "Counter")
        .expect("Counter userdata");
    let get_method = counter
        .methods
        .iter()
        .find(|m| m.name == "get")
        .expect("get method");
    k9::assert_equal!(get_method.doc, Some("Read the current count.".to_owned()));
}
