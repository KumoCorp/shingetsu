//! End-to-end tests for the markdown emitter.

use std::collections::HashMap;
use std::path::PathBuf;

use shingetsu::{module, userdata};
use shingetsu_docgen::{
    extract, render_markdown, DocModel, FrontMatterStyle, MdFile, MdOptions, SplitMode,
};
use shingetsu_vm::GlobalEnv;

/// A counter that returns itself from `clone`, exercising cross-page
/// type linking.
struct Counter(#[allow(dead_code)] i64);

/// A counter that returns itself from `clone`.
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

/// A small utility module.
#[module(name = "smallmath")]
#[allow(dead_code)]
mod smallmath_impl {
    /// Format-time version string.
    #[field]
    fn version() -> String {
        "1.0".to_owned()
    }

    /// Return the larger of two numbers.
    ///
    /// # Parameters
    ///
    /// - `a` — first value
    /// - `b` — second value
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
}

fn build_env() -> GlobalEnv {
    let env = GlobalEnv::new();
    smallmath_impl::register_preload(&env);
    env.register_userdata_type(Counter::userdata_type());
    env
}

fn find<'a>(files: &'a [MdFile], path: &str) -> &'a MdFile {
    files
        .iter()
        .find(|f| f.path == PathBuf::from(path))
        .unwrap_or_else(|| panic!("expected {path} in output"))
}

#[test]
fn every_item_is_addressable_inline() {
    let model = extract(&build_env());
    let files = render_markdown(&model, &MdOptions::default());
    k9::assert_equal!(
        collect_urls(&files),
        vec![
            "index.md",
            "modules/smallmath/index.md",
            "modules/smallmath/index.md#field-version",
            "modules/smallmath/index.md#function-max",
            "types/Counter/index.md",
            "types/Counter/index.md#field-value",
            "types/Counter/index.md#function-increment",
        ]
    );
}

#[test]
fn every_item_is_addressable_split() {
    let model = extract(&build_env());
    let opts = MdOptions {
        split_threshold: 0,
        ..MdOptions::default()
    };
    let files = render_markdown(&model, &opts);
    k9::assert_equal!(
        collect_urls(&files),
        vec![
            "index.md",
            "modules/smallmath/index.md",
            "modules/smallmath/version.md",
            "modules/smallmath/max.md",
            "types/Counter/index.md",
            "types/Counter/value.md",
            "types/Counter/increment.md",
        ]
    );
}

#[test]
fn cross_page_type_links_emitted() {
    // Build a synthetic model with a function whose param type is a
    // userdata reference, to exercise the linking logic regardless of
    // the macro-generated content.
    use shingetsu_docgen::{
        FieldDoc, FieldDocKind, FunctionDoc, ModuleDoc, ReturnDoc, TypeRef, UserdataDoc,
        SCHEMA_VERSION,
    };
    let counter_ref = TypeRef::Named {
        name: "Counter".into(),
    };
    let model = DocModel {
        schema_version: SCHEMA_VERSION,
        modules: vec![ModuleDoc {
            name: "factory".into(),
            doc: None,
            strict: false,
            fields: vec![],
            functions: vec![FunctionDoc {
                name: "make".into(),
                doc: None,
                synopsis: "factory.make() -> Counter".into(),
                params: vec![],
                variadic: None,
                returns: vec![ReturnDoc {
                    ty: counter_ref.clone(),
                    doc: None,
                }],
                is_method: false,
            }],
        }],
        userdata_types: vec![UserdataDoc {
            name: "Counter".into(),
            doc: None,
            fields: vec![FieldDoc {
                name: "value".into(),
                doc: None,
                ty: TypeRef::Number,
                kind: FieldDocKind::Getter,
            }],
            methods: vec![],
            metamethods: vec![],
        }],
        globals: vec![],
    };
    let files = render_markdown(&model, &MdOptions::default());
    let factory_page = &find(&files, "modules/factory/index.md").content;
    k9::assert_equal!(
        factory_page,
        "# factory\n\n## Functions\n\n### make {#function-make}\n\n```\nfactory.make() -> Counter\n```\n\n**Returns**\n\n- [Counter](../../types/Counter/index.md)\n\n"
    );
}

#[test]
fn inline_snapshot() {
    let model = extract(&build_env());
    let files = render_markdown(&model, &MdOptions::default());
    let actual = stringify_files(&files);
    let expected = include_str!("fixtures/markdown_inline.txt");
    k9::assert_equal!(actual.trim_end(), expected.trim_end());
}

#[test]
fn split_snapshot() {
    let model = extract(&build_env());
    let opts = MdOptions {
        split_threshold: 0,
        ..MdOptions::default()
    };
    let files = render_markdown(&model, &opts);
    let actual = stringify_files(&files);
    let expected = include_str!("fixtures/markdown_split.txt");
    k9::assert_equal!(actual.trim_end(), expected.trim_end());
}

#[test]
fn split_overrides_force_specific_layout() {
    let model = extract(&build_env());
    let mut overrides = HashMap::new();
    overrides.insert("Counter".into(), SplitMode::Split);
    let opts = MdOptions {
        split_overrides: overrides,
        ..MdOptions::default()
    };
    let files = render_markdown(&model, &opts);
    let paths: Vec<PathBuf> = files.iter().map(|f| f.path.clone()).collect();
    k9::assert_equal!(
        paths,
        vec![
            PathBuf::from("index.md"),
            PathBuf::from("modules/smallmath/index.md"),
            PathBuf::from("types/Counter/index.md"),
            PathBuf::from("types/Counter/value.md"),
            PathBuf::from("types/Counter/increment.md"),
        ]
    );
}

#[test]
fn front_matter_zensical_emitted() {
    let model = extract(&build_env());
    let opts = MdOptions {
        front_matter: FrontMatterStyle::Zensical,
        ..MdOptions::default()
    };
    let files = render_markdown(&model, &opts);
    k9::assert_equal!(
        find(&files, "index.md").content,
        "---\ntitle: Reference\n---\n\n# Reference\n\n## Modules\n\n- [`smallmath`](modules/smallmath/index.md) — A small utility module.\n\n## Types\n\n- [`Counter`](types/Counter/index.md) — A counter that returns itself from `clone`.\n\n"
    );
}

#[test]
fn json_round_trip_produces_identical_markdown() {
    let model = extract(&build_env());
    let direct = render_markdown(&model, &MdOptions::default());

    let json = serde_json::to_string(&model).expect("serialize");
    let parsed: DocModel = serde_json::from_str(&json).expect("deserialize");
    let via_json = render_markdown(&parsed, &MdOptions::default());

    k9::assert_equal!(direct, via_json);
}

fn stringify_files(files: &[MdFile]) -> String {
    let mut out = String::new();
    for f in files {
        out.push_str(&format!("=== {} ===\n", f.path.display()));
        out.push_str(&f.content);
        if !f.content.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

/// Walk emitted files and produce the full set of addressable URLs:
/// each file path, plus `path#anchor` for every `{#anchor}` marker
/// found inside the file.  Order matches the file emission order so
/// the result can be compared against an explicit expected list.
fn collect_urls(files: &[MdFile]) -> Vec<String> {
    let mut out = Vec::new();
    for f in files {
        let path = f.path.display().to_string();
        out.push(path.clone());
        let mut rest = f.content.as_str();
        while let Some(open) = rest.find("{#") {
            rest = &rest[open + 2..];
            let close = match rest.find('}') {
                Some(i) => i,
                None => break,
            };
            let anchor = &rest[..close];
            out.push(format!("{path}#{anchor}"));
            rest = &rest[close + 1..];
        }
    }
    out
}
