//! End-to-end extraction tests for `shingetsu-docgen`.

use shingetsu::{module, userdata};
use shingetsu_docgen::{
    extract, render_luau, DocModel, FieldDoc, FieldDocKind, FunctionDoc, ModuleDoc, ParamDoc,
    ReturnDoc, TypeRef, UserdataDoc, SCHEMA_VERSION,
};
use shingetsu_vm::GlobalEnv;

/// A counter exposed as userdata.
struct Counter(#[allow(dead_code)] i64);

/// A counter exposed as userdata.
#[userdata]
impl Counter {
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

/// A small math module.
#[module(name = "smallmath")]
#[allow(dead_code)]
mod smallmath_impl {
    /// Return the larger of two numbers.
    ///
    /// # Parameters
    ///
    /// - `a` — the first value
    /// - `b` — the second value
    ///
    /// # Returns
    ///
    /// - the larger of `a` and `b`
    #[function]
    fn max(a: f64, b: f64) -> f64 {
        if a > b {
            a
        } else {
            b
        }
    }

    /// Format-time version string.
    #[field]
    fn version() -> String {
        "1.0".to_owned()
    }
}

fn ty_named(s: &str) -> TypeRef {
    TypeRef {
        display: s.to_owned(),
        references: vec![s.to_owned()],
    }
}

fn ty_plain(s: &str) -> TypeRef {
    TypeRef {
        display: s.to_owned(),
        references: vec![],
    }
}

fn build_env() -> GlobalEnv {
    let env = GlobalEnv::new();
    smallmath_impl::register_preload(&env);
    env.register_userdata_type(Counter::userdata_type());
    env
}

fn expected_model() -> DocModel {
    DocModel {
        schema_version: SCHEMA_VERSION,
        modules: vec![ModuleDoc {
            name: "smallmath".into(),
            doc: Some("A small math module.".into()),
            strict: false,
            fields: vec![FieldDoc {
                name: "version".into(),
                doc: Some("Format-time version string.".into()),
                ty: ty_plain("string"),
                kind: FieldDocKind::Eager,
            }],
            functions: vec![FunctionDoc {
                name: "max".into(),
                doc: Some("Return the larger of two numbers.".into()),
                synopsis: "smallmath.max(a, b) -> number".into(),
                params: vec![
                    ParamDoc {
                        name: Some("a".into()),
                        ty: ty_plain("number"),
                        optional: false,
                        doc: Some("the first value".into()),
                    },
                    ParamDoc {
                        name: Some("b".into()),
                        ty: ty_plain("number"),
                        optional: false,
                        doc: Some("the second value".into()),
                    },
                ],
                variadic: None,
                returns: vec![ReturnDoc {
                    ty: ty_plain("number"),
                    doc: Some("the larger of `a` and `b`".into()),
                }],
                is_method: false,
            }],
        }],
        userdata_types: vec![UserdataDoc {
            name: "Counter".into(),
            doc: Some("A counter exposed as userdata.".into()),
            fields: vec![FieldDoc {
                name: "value".into(),
                doc: Some("The current count.".into()),
                ty: ty_plain("number"),
                kind: FieldDocKind::Getter,
            }],
            methods: vec![FunctionDoc {
                name: "increment".into(),
                doc: Some("Add `amount` to the counter and return the new value.".into()),
                synopsis: "Counter:increment(amount) -> number".into(),
                params: vec![ParamDoc {
                    name: Some("amount".into()),
                    ty: ty_plain("number"),
                    optional: false,
                    doc: Some("the number to add".into()),
                }],
                variadic: None,
                returns: vec![ReturnDoc {
                    ty: ty_plain("number"),
                    doc: Some("the new value of the counter".into()),
                }],
                is_method: true,
            }],
            metamethods: vec![],
        }],
        globals: vec![],
    }
}

#[test]
fn extract_produces_expected_doc_model() {
    let env = build_env();
    k9::assert_equal!(extract(&env), expected_model());
}

#[test]
fn typeref_named_type_is_collected() {
    // Sanity: a Named type collects the reference for cross-linking.
    let _ = ty_named("Counter");
    let r = TypeRef::from_lua_type(&shingetsu_vm::LuaType::Named("Counter".into()));
    k9::assert_equal!(
        r,
        TypeRef {
            display: "Counter".into(),
            references: vec!["Counter".into()],
        }
    );
}

#[test]
fn doc_model_round_trips_through_json() {
    let model = expected_model();
    let json = serde_json::to_string_pretty(&model).expect("serialize");
    let parsed: DocModel = serde_json::from_str(&json).expect("deserialize");
    k9::assert_equal!(parsed, model);
}

#[test]
fn doc_model_json_snapshot() {
    let json = serde_json::to_string_pretty(&extract(&build_env())).expect("serialize");
    let expected = include_str!("fixtures/doc_model.json");
    k9::assert_equal!(json, expected.trim_end());
}

#[test]
fn luau_definitions_snapshot() {
    let actual = render_luau(&extract(&build_env()));
    let expected = include_str!("fixtures/definitions.d.luau");
    k9::assert_equal!(actual.trim_end(), expected.trim_end());
}
