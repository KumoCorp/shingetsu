//! End-to-end tests for the markdown emitter.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use shingetsu::{module, userdata};
use shingetsu_docgen::{
    render_markdown, render_nav_fragment, DocModel, EventDoc, FieldDoc, FieldDocKind,
    FrontMatterStyle, FunctionDoc, MdFile, MdOptions, MetamethodDoc, ModuleDoc, ParamDoc,
    SplitMode, TypeRef, UserdataDoc,
};
use shingetsu_vm::GlobalEnv;

mod common;
use common::extract;

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
        .find(|f| f.path.as_path() == Path::new(path))
        .unwrap_or_else(|| panic!("expected {path} in output"))
}

/// Module that exercises a parameter type containing both a
/// linkable userdata reference (`Counter`) and primitive labels
/// like `integer` that look like markdown shortcut-reference
/// link syntax (`[integer]`) once the type is rendered.
#[module(name = "linkescape")]
#[allow(dead_code)]
mod linkescape_impl {
    use super::Counter;
    /// Take an array of counters.
    ///
    /// # Parameters
    ///
    /// - `xs` — the array
    #[function]
    fn run(xs: Vec<shingetsu_vm::Ud<Counter>>) -> i64 {
        xs.len() as i64
    }
}

#[test]
fn type_link_escapes_brackets_in_unlinked_text() {
    // The rendered type is `{[integer]: Counter}` with a link
    // inserted around `Counter`.  The literal `[integer]` must be
    // escaped so CommonMark doesn't treat it as a reference-style
    // link label and emit an "unresolved link reference" warning.
    let env = GlobalEnv::new();
    linkescape_impl::register_preload(&env);
    env.register_userdata_type(Counter::userdata_type());
    let model = shingetsu_docgen::extract(&env);
    let opts = MdOptions {
        split_threshold: 0,
        ..MdOptions::default()
    };
    let files = render_markdown(&model, &opts);
    let page = find(&files, "modules/linkescape/run.md");
    let line: &str = page
        .content
        .lines()
        .find(|l: &&str| l.starts_with("- `xs`"))
        .expect("parameter line for xs");
    k9::assert_equal!(
        line,
        "- `xs`: {\\[integer\\]: [Counter](../../types/Counter/index.md)} — the array"
    );
}

#[test]
fn every_item_is_addressable_default() {
    // Modules always split (one page per item).  Userdata types
    // stay inline when they fit under `split_threshold`.
    let model = extract(&build_env());
    let files = render_markdown(&model, &MdOptions::default());
    k9::assert_equal!(
        collect_urls(&files),
        vec![
            "index.md",
            "modules/smallmath/index.md",
            "modules/smallmath/version.md",
            "modules/smallmath/max.md",
            "types/Counter/index.md",
            "types/Counter/index.md#field-value",
            "types/Counter/index.md#function-increment",
        ]
    );
}

#[test]
fn every_item_is_addressable_split_userdata() {
    // `split_threshold: 0` forces userdata types to split too,
    // matching the always-split behaviour of modules.
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
fn events_render_index_and_per_event_pages() {
    use shingetsu_docgen::SCHEMA_VERSION;
    let model = DocModel {
        schema_version: SCHEMA_VERSION,
        modules: vec![],
        userdata_types: vec![],
        globals: vec![],
        events: vec![EventDoc {
            name: "on_reset".into(),
            doc: Some("Fired when a queue is reset.".into()),
            synopsis: "on_reset(queue, manual) -> boolean".into(),
            params: vec![
                ParamDoc {
                    name: Some("queue".into()),
                    ty: TypeRef::String,
                    optional: false,
                    doc: Some("the queue being reset".into()),
                },
                ParamDoc {
                    name: Some("manual".into()),
                    ty: TypeRef::Boolean,
                    optional: false,
                    doc: None,
                },
            ],
            returns: vec![TypeRef::Boolean],
            return_doc: Some("`true` to allow the reset.".into()),
        }],
    };
    let files = render_markdown(&model, &MdOptions::default());
    let paths: Vec<&str> = files.iter().map(|f| f.path.to_str().unwrap()).collect();
    k9::assert_equal!(
        paths,
        vec!["index.md", "events/index.md", "events/on_reset.md"]
    );
    let index = &find(&files, "index.md").content;
    k9::assert_equal!(
        index,
        "# Reference\n\n## Events\n\n- [All events](events/index.md)\n\n"
    );
    let events_index = &find(&files, "events/index.md").content;
    k9::assert_equal!(
        events_index,
        "# Events\n\n- [`on_reset(queue, manual) -> boolean`](on_reset.md) \u{2014} Fired when a queue is reset.\n\n"
    );
    let event_page = &find(&files, "events/on_reset.md").content;
    k9::assert_equal!(
        event_page,
        "# on_reset\n\n\
         ```\n\
         on_reset(queue, manual) -> boolean\n\
         ```\n\n\
         Fired when a queue is reset.\n\n\
         **Parameters**\n\n\
         - `queue`: `string` \u{2014} the queue being reset\n\
         - `manual`: `boolean`\n\n\
         **Returns**\n\n\
         - `boolean` -- `true` to allow the reset.\n\n"
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
                variadic_doc: None,
                examples: vec![],
                deprecated: None,
                must_use: None,
            }],
            partial: false,
            deprecated: None,
        }],
        userdata_types: vec![UserdataDoc {
            name: "Counter".into(),
            doc: None,
            fields: vec![FieldDoc {
                name: "value".into(),
                doc: None,
                ty: TypeRef::Number,
                kind: FieldDocKind::Getter,
                examples: vec![],
                deprecated: None,
            }],
            methods: vec![],
            metamethods: vec![],
            partial: false,
        }],
        globals: vec![],
        events: vec![],
    };
    let files = render_markdown(&model, &MdOptions::default());
    // Modules always split, so the cross-link lives on the per-item
    // page (not the parent index).
    let make_page = &find(&files, "modules/factory/make.md").content;
    k9::assert_equal!(
        make_page,
        "# factory.make\n\n```\nfactory.make() -> Counter\n```\n\n**Returns**\n\n- [Counter](../../types/Counter/index.md)\n\n"
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
fn split_overrides_force_userdata_layout() {
    // `split_overrides` can force a specific userdata type to split.
    // Modules always split regardless.
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
            PathBuf::from("modules/smallmath/version.md"),
            PathBuf::from("modules/smallmath/max.md"),
            PathBuf::from("types/Counter/index.md"),
            PathBuf::from("types/Counter/value.md"),
            PathBuf::from("types/Counter/increment.md"),
        ]
    );
}

#[test]
fn function_page_front_matter_title_omits_synopsis() {
    // The title field on function pages must be the short
    // qualified name (e.g. `smallmath.max`), not the full
    // synopsis — type expressions in the synopsis carry
    // bracket-bearing tokens like `[integer]` that downstream
    // YAML consumers parse as markdown reference link labels.
    let model = extract(&build_env());
    let opts = MdOptions {
        split_threshold: 0,
        front_matter: FrontMatterStyle::Zensical,
        ..MdOptions::default()
    };
    let files = render_markdown(&model, &opts);
    let head: String = find(&files, "modules/smallmath/max.md")
        .content
        .lines()
        .take(3)
        .collect::<Vec<&str>>()
        .join("\n");
    k9::assert_equal!(head, "---\ntitle: 'smallmath.max'\n---");
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
        "---\ntitle: 'Reference'\n---\n\n# Reference\n\n## Modules\n\n- [`smallmath`](modules/smallmath/index.md) — A small utility module.\n\n## Types\n\n- [`Counter`](types/Counter/index.md) — A counter that returns itself from `clone`.\n\n"
    );
}

/// Build a DocModel where every list (modules, types, fields,
/// functions, methods, metamethods) is in deliberately non-alpha
/// declaration order, so renderers that respect source order would
/// produce visibly out-of-order output.
fn unsorted_model() -> DocModel {
    let mk_field = |name: &str| FieldDoc {
        name: name.into(),
        doc: Some(format!("docs for {name}")),
        ty: TypeRef::Number,
        kind: FieldDocKind::Getter,
        examples: vec![],
        deprecated: None,
    };
    let mk_func = |name: &str| FunctionDoc {
        name: name.into(),
        doc: Some(format!("docs for {name}")),
        synopsis: format!("{name}()"),
        params: vec![],
        variadic: None,
        variadic_doc: None,
        returns: vec![],
        is_method: false,
        examples: vec![],
        deprecated: None,
        must_use: None,
    };
    let mk_meth = |name: &str| FunctionDoc {
        name: name.into(),
        doc: Some(format!("docs for {name}")),
        synopsis: format!("self:{name}()"),
        params: vec![],
        variadic: None,
        variadic_doc: None,
        returns: vec![],
        is_method: true,
        examples: vec![],
        deprecated: None,
        must_use: None,
    };
    let mk_meta = |method: &str| MetamethodDoc {
        method: method.into(),
        doc: Some(format!("docs for {method}")),
        synopsis: format!("{method}(self)"),
        params: vec![],
        variadic: None,
        variadic_doc: None,
        returns: vec![],
        examples: vec![],
    };
    DocModel {
        schema_version: shingetsu_docgen::SCHEMA_VERSION,
        modules: vec![
            ModuleDoc {
                name: "zoo".into(),
                doc: Some("zoo module".into()),
                strict: false,
                fields: vec![mk_field("yolk"), mk_field("apple")],
                functions: vec![mk_func("yawn"), mk_func("bark")],
                partial: false,
                deprecated: None,
            },
            ModuleDoc {
                name: "alpha".into(),
                doc: Some("alpha module".into()),
                strict: false,
                fields: vec![],
                functions: vec![mk_func("zip"), mk_func("add")],
                partial: false,
                deprecated: None,
            },
        ],
        userdata_types: vec![
            UserdataDoc {
                name: "Zoo".into(),
                doc: Some("Zoo type".into()),
                fields: vec![mk_field("yolk"), mk_field("apple")],
                methods: vec![mk_meth("yodel"), mk_meth("bark")],
                metamethods: vec![mk_meta("__newindex"), mk_meta("__index")],
                partial: false,
            },
            UserdataDoc {
                name: "Alpha".into(),
                doc: Some("Alpha type".into()),
                fields: vec![],
                methods: vec![],
                metamethods: vec![],
                partial: false,
            },
        ],
        globals: vec![],
        events: vec![],
    }
}

#[test]
fn lists_are_alpha_sorted_in_index() {
    let files = render_markdown(&unsorted_model(), &MdOptions::default());
    let index = &find(&files, "index.md").content;
    k9::assert_equal!(
        index,
        "# Reference\n\n## Modules\n\n- [`alpha`](modules/alpha/index.md) — alpha module\n- [`zoo`](modules/zoo/index.md) — zoo module\n\n## Types\n\n- [`Alpha`](types/Alpha/index.md) — Alpha type\n- [`Zoo`](types/Zoo/index.md) — Zoo type\n\n"
    );
}

#[test]
fn lists_are_alpha_sorted_in_module_page() {
    let files = render_markdown(&unsorted_model(), &MdOptions::default());
    let zoo = &find(&files, "modules/zoo/index.md").content;
    k9::assert_equal!(
        zoo,
        "# zoo\n\nzoo module\n\n## Fields\n\n- [`apple`](apple.md) — docs for apple\n- [`yolk`](yolk.md) — docs for yolk\n\n## Functions\n\n- [`bark()`](bark.md) — docs for bark\n- [`yawn()`](yawn.md) — docs for yawn\n\n"
    );
}

#[test]
fn lists_are_alpha_sorted_in_userdata_page() {
    // Force split so the per-userdata page emits index entries.
    let opts = MdOptions {
        split_threshold: 0,
        ..MdOptions::default()
    };
    let files = render_markdown(&unsorted_model(), &opts);
    let zoo = &find(&files, "types/Zoo/index.md").content;
    k9::assert_equal!(
        zoo,
        "# Zoo\n\nZoo type\n\n## Fields\n\n- [`apple`](apple.md) — docs for apple\n- [`yolk`](yolk.md) — docs for yolk\n\n## Methods\n\n- [`self:bark()`](bark.md) — docs for bark\n- [`self:yodel()`](yodel.md) — docs for yodel\n\n## Metamethods\n\n- [`__index(self)`](__index.md) — docs for __index\n- [`__newindex(self)`](__newindex.md) — docs for __newindex\n\n"
    );
}

#[test]
fn nav_fragment_is_alpha_sorted() {
    let fragment = render_nav_fragment(&unsorted_model(), &MdOptions::default(), "reference");
    k9::assert_equal!(
        fragment,
        r#"{ "Reference" = [
  "reference/index.md",
  { "Modules" = [
    { "alpha" = [
      "reference/modules/alpha/index.md",
      { "alpha.add" = "reference/modules/alpha/add.md" },
      { "alpha.zip" = "reference/modules/alpha/zip.md" },
    ] },
    { "zoo" = [
      "reference/modules/zoo/index.md",
      { "zoo.apple" = "reference/modules/zoo/apple.md" },
      { "zoo.yolk" = "reference/modules/zoo/yolk.md" },
      { "zoo.bark" = "reference/modules/zoo/bark.md" },
      { "zoo.yawn" = "reference/modules/zoo/yawn.md" },
    ] },
  ] },
  { "Types" = [
    { "Alpha" = "reference/types/Alpha/index.md" },
    { "Zoo" = "reference/types/Zoo/index.md" },
  ] },
] }
"#
    );
}

#[test]
fn nav_fragment_inline_userdata() {
    let model = extract(&build_env());
    let fragment = render_nav_fragment(&model, &MdOptions::default(), "reference");
    k9::assert_equal!(
        fragment,
        r#"{ "Reference" = [
  "reference/index.md",
  { "Modules" = [
    { "smallmath" = [
      "reference/modules/smallmath/index.md",
      { "smallmath.version" = "reference/modules/smallmath/version.md" },
      { "smallmath.max" = "reference/modules/smallmath/max.md" },
    ] },
  ] },
  { "Types" = [
    { "Counter" = "reference/types/Counter/index.md" },
  ] },
] }
"#
    );
}

#[test]
fn nav_fragment_split_userdata_expands_items() {
    let model = extract(&build_env());
    let opts = MdOptions {
        split_threshold: 0,
        ..MdOptions::default()
    };
    let fragment = render_nav_fragment(&model, &opts, "reference");
    k9::assert_equal!(
        fragment,
        r#"{ "Reference" = [
  "reference/index.md",
  { "Modules" = [
    { "smallmath" = [
      "reference/modules/smallmath/index.md",
      { "smallmath.version" = "reference/modules/smallmath/version.md" },
      { "smallmath.max" = "reference/modules/smallmath/max.md" },
    ] },
  ] },
  { "Types" = [
    { "Counter" = [
      "reference/types/Counter/index.md",
      { "Counter.value" = "reference/types/Counter/value.md" },
      { "Counter.increment" = "reference/types/Counter/increment.md" },
    ] },
  ] },
] }
"#
    );
}

#[test]
fn nav_fragment_empty_prefix() {
    let model = extract(&build_env());
    let fragment = render_nav_fragment(&model, &MdOptions::default(), "");
    // Paths should not have a leading slash when prefix is empty.
    k9::assert_equal!(
        fragment,
        r#"{ "Reference" = [
  "index.md",
  { "Modules" = [
    { "smallmath" = [
      "modules/smallmath/index.md",
      { "smallmath.version" = "modules/smallmath/version.md" },
      { "smallmath.max" = "modules/smallmath/max.md" },
    ] },
  ] },
  { "Types" = [
    { "Counter" = "types/Counter/index.md" },
  ] },
] }
"#
    );
}

#[test]
fn nav_fragment_includes_events_subtree() {
    use shingetsu_docgen::SCHEMA_VERSION;
    let model = DocModel {
        schema_version: SCHEMA_VERSION,
        modules: vec![],
        userdata_types: vec![],
        globals: vec![],
        events: vec![
            EventDoc {
                name: "on_reset".into(),
                doc: None,
                synopsis: "on_reset(queue) -> boolean".into(),
                params: vec![],
                returns: vec![TypeRef::Boolean],
                return_doc: None,
            },
            EventDoc {
                name: "before_migrate".into(),
                doc: None,
                synopsis: "before_migrate(tenant) -> boolean".into(),
                params: vec![],
                returns: vec![TypeRef::Boolean],
                return_doc: None,
            },
        ],
    };
    let fragment = render_nav_fragment(&model, &MdOptions::default(), "reference");
    k9::assert_equal!(
        fragment,
        r#"{ "Reference" = [
  "reference/index.md",
  { "Events" = [
    "reference/events/index.md",
    { "before_migrate" = "reference/events/before_migrate.md" },
    { "on_reset" = "reference/events/on_reset.md" },
  ] },
] }
"#
    );
}

#[test]
fn nav_fragment_parses_as_toml() {
    let model = extract(&build_env());
    let fragment = render_nav_fragment(&model, &MdOptions::default(), "reference");
    // Substitute into a synthetic config to verify the result is
    // valid TOML and that zensical's nav structure accepts it.
    let config = format!("nav = [ {fragment} ]");
    let parsed: toml::Value = toml::from_str(&config).expect("fragment must be valid TOML");
    let nav = parsed
        .get("nav")
        .and_then(|v| v.as_array())
        .expect("nav array");
    k9::assert_equal!(nav.len(), 1);
    let reference = nav[0]
        .as_table()
        .and_then(|t| t.get("Reference"))
        .and_then(|v| v.as_array())
        .expect("Reference array");
    k9::assert_equal!(reference[0].as_str(), Some("reference/index.md"));
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
