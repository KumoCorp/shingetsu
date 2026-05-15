mod common;

use common::write_temp_file;

use shingetsu::diagnostic::{render_warnings, RenderStyle};
use shingetsu::lint_plugin::{
    load_plugin, new_plugin_env, registry, LoadedPlugins, PluginDeclaration, Severity,
    SCHEMA_VERSION,
};
use shingetsu_compiler::{CompileOptions, Compiler};
use shingetsu_vm::GlobalEnv;
use std::sync::Arc;

/// `lint.declare` and `lint.on` round-trip through the registry.
#[tokio::test]
async fn load_minimal_plugin_records_declaration() {
    let env = new_plugin_env().expect("new env");
    let plugin = write_temp_file(
        r#"
local lint = require("shingetsu.lint")
lint.declare {
    name = "demo",
    description = "demo plugin",
}
lint.on("method_call", function() end)
lint.on("function_call", function() end)
"#,
    );
    let decl = load_plugin(&env, plugin.path()).await.expect("load");
    let expected_decl = PluginDeclaration {
        name: "demo".into(),
        description: "demo plugin".into(),
        default_severity: Severity::Warning,
        sets: vec![],
        min_schema: None,
        source_path: plugin.path().to_path_buf(),
        declare_call_site: decl.declare_call_site.clone(),
    };
    k9::assert_equal!(decl, expected_decl);
    let reg = registry(&env);
    k9::assert_equal!(reg.declarations(), vec![expected_decl]);
}

/// `lint.on` may appear before `lint.declare`.
#[tokio::test]
async fn declare_after_on_is_harmless() {
    let env = new_plugin_env().expect("new env");
    let plugin = write_temp_file(
        r#"
local lint = require("shingetsu.lint")
lint.on("method_call", function() end)
lint.declare {
    name = "late_declare",
    description = "registration ordering",
}
"#,
    );
    let decl = load_plugin(&env, plugin.path()).await.expect("load");
    k9::assert_equal!(decl.name, "late_declare");
}

#[tokio::test]
async fn duplicate_declare_in_same_file_errors() {
    common::assert_plugin_load_error!(
        r#"
local lint = require("shingetsu.lint")
lint.declare { name = "first", description = "1" }
lint.declare { name = "second", description = "2" }
"#,
        concat!(
            r#"error: lint.declare called more than once in the same plugin file
 --> <plugin>:4:1
  |
4 | lint.declare { name = "second", description = "2" }
  | ^^^^^^^^^^^^ lint.declare called more than once in the same plugin file
stack traceback:"#,
            "\n\t<plugin>:4: in main chunk",
        )
    );
}

#[tokio::test]
async fn missing_declare_errors() {
    common::assert_plugin_load_error!(
        r#"
local lint = require("shingetsu.lint")
lint.on("method_call", function() end)
"#,
        "plugin file <plugin> never called `lint.declare {...}`"
    );
}

#[tokio::test]
async fn invalid_lint_name_errors() {
    common::assert_plugin_load_error!(
        r#"
local lint = require("shingetsu.lint")
lint.declare { name = "BadName", description = "x" }
"#,
        concat!(
            r#"error: bad argument #1 to 'declare' (validated name expected, got lint name 'BadName' must be snake_case ASCII (lowercase letters, digits, underscores))
 --> <plugin>:3:14
  |
3 | lint.declare { name = "BadName", description = "x" }
  |              ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ bad argument #1 to 'declare' (validated name expected, got lint name 'BadName' must be snake_case ASCII (lowercase letters, digits, underscores))
stack traceback:"#,
            "\n\t<plugin>:3: in main chunk",
        )
    );
}

/// Unknown event names are rejected by the callback registry's closed name
/// policy with a did-you-mean suggestion.
#[tokio::test]
async fn unknown_event_name_is_rejected() {
    common::assert_plugin_load_error!(
        r#"
local lint = require("shingetsu.lint")
lint.declare { name = "demo", description = "d" }
lint.on("function_callz", function() end)
"#,
        concat!(
            r#"error: error in 'callback': 'function_callz' is not a recognised event name. Did you mean one of `function_call`, `function_decl`, `function_expr`? There are too many alternatives to list here; consult the documentation!
 --> <plugin>:4:1
  |
4 | lint.on("function_callz", function() end)
  | ^^^^^^^ error in 'callback': 'function_callz' is not a recognised event name. Did you mean one of `function_call`, `function_decl`, `function_expr`? There are too many alternatives to list here; consult the documentation!
stack traceback:"#,
            "\n\t<plugin>:4: in main chunk",
        )
    );
}

/// `min_schema` higher than the host's `SCHEMA_VERSION` prevents load.
#[tokio::test]
async fn min_schema_too_high_is_load_error() {
    let src = format!(
        "local lint = require(\"shingetsu.lint\")\n\
         lint.declare {{ name = \"demo\", description = \"d\", \
         min_schema = {} }}",
        SCHEMA_VERSION + 1,
    );
    common::assert_plugin_load_error!(
        &src,
        format!(
            concat!(
                "error: plugin 'demo' requires schema version {next} but this host provides version {cur}\n",
                " --> <plugin>:2:1\n",
                "  |\n",
                "2 | lint.declare {{ name = \"demo\", description = \"d\", min_schema = {next} }}\n",
                "  | ^^^^^^^^^^^^ plugin 'demo' requires schema version {next} but this host provides version {cur}\n",
                "stack traceback:\n",
                "\t<plugin>:2: in main chunk",
            ),
            next = SCHEMA_VERSION + 1,
            cur = SCHEMA_VERSION,
        )
    );
}

/// `min_schema` equal to the host's `SCHEMA_VERSION` loads successfully.
#[tokio::test]
async fn min_schema_at_host_version_is_ok() {
    let src = format!(
        "local lint = require(\"shingetsu.lint\")\n\
         lint.declare {{ name = \"demo\", description = \"d\", \
         min_schema = {} }}",
        SCHEMA_VERSION,
    );
    let plugin = write_temp_file(&src);
    load_plugin(&new_plugin_env().unwrap(), plugin.path())
        .await
        .expect("plugin with min_schema == SCHEMA_VERSION should load");
}

/// `lint.schema_version` is an integer field readable from plugin code.
#[tokio::test]
async fn schema_version_exposed_on_module() {
    common::assert_plugin_diagnostics!(
        r#"
local lint = require("shingetsu.lint")
lint.declare { name = "schema_ver", description = "d" }
lint.on("return", function(stmt, ctx)
    local v = lint.schema_version
    if type(v) ~= "number" then
        ctx:warn(stmt.span, "SCHEMA_VERSION is not a number: " .. tostring(v))
    end
end)
"#,
        "return nil",
        "",
    );
}

/// A plugin handler that emits `ctx:warn` from a `method_call` event
/// produces a rendered diagnostic anchored at the call's span.
#[tokio::test]
async fn method_call_event_fires_and_warn_collects_diagnostic() {
    common::assert_plugin_diagnostics!(
        r#"
local lint = require("shingetsu.lint")
lint.declare { name = "demo", description = "d" }
lint.on("method_call", function(call, ctx)
    ctx:warn(call.span, "saw method " .. call.method)
end)
"#,
        "obj:foo()",
        r#"warning[project:demo]: saw method foo
 --> test.lua:1:1
  |
1 | obj:foo()
  | ^^^^^^^^ saw method foo"#,
    );
}

#[tokio::test]
async fn function_call_event_fires() {
    common::assert_plugin_diagnostics!(
        r#"
local lint = require("shingetsu.lint")
lint.declare { name = "demo", description = "d" }
lint.on("function_call", function(call, ctx)
    ctx:warn(call.span, "saw function_call")
end)
"#,
        "print(1)",
        r#"warning[project:demo]: saw function_call
 --> test.lua:1:1
  |
1 | print(1)
  | ^^^^^^^^ saw function_call"#,
    );
}

/// A `function_call` fired from inside a `---`-doc-commented statement
/// sees the enclosing doc text via `call.doc_comment`.
#[tokio::test]
async fn function_call_inherits_enclosing_doc_comment() {
    common::assert_plugin_diagnostics!(
        r#"
local lint = require("shingetsu.lint")
lint.declare { name = "demo", description = "d" }
lint.on("function_call", function(call, ctx)
    local doc = call.doc_comment
    if doc then
        ctx:warn(call.span, "doc=" .. doc)
    end
end)
"#,
        "--- hello\nlocal x = f()",
        r#"warning[project:demo]: doc=hello
 --> test.lua:2:11
  |
2 | local x = f()
  |           ^^ doc=hello"#,
    );
}

#[tokio::test]
async fn assign_event_fires() {
    common::assert_plugin_diagnostics!(
        r#"
local lint = require("shingetsu.lint")
lint.declare { name = "demo", description = "d" }
lint.on("assign", function(node, ctx)
    ctx:warn(node.span, "saw assign")
end)
"#,
        "x = 1",
        r#"warning[project:demo]: saw assign
 --> test.lua:1:1
  |
1 | x = 1
  | ^^^^^ saw assign"#,
    );
}

/// A handler error is caught and converted to a `Warning` at the
/// visited node's span; remaining events still fire.
#[tokio::test]
async fn handler_error_becomes_warning_and_walk_continues() {
    common::assert_plugin_diagnostics!(
        r#"
local lint = require("shingetsu.lint")
lint.declare { name = "demo", description = "d" }
lint.on("method_call", function(call, ctx)
    if call.method == "bad" then
        error("boom")
    else
        ctx:warn(call.span, "hi from " .. call.method)
    end
end)
"#,
        "obj:bad() obj:good()",
        r#"warning[project:demo]: lint plugin 'demo' handler raised: <plugin>:6: boom
 --> test.lua:1:1
  |
1 | obj:bad() obj:good()
  | ^^^^^^^^ lint plugin 'demo' handler raised: <plugin>:6: boom
warning[project:demo]: hi from good
 --> test.lua:1:11
  |
1 | obj:bad() obj:good()
  |           ^^^^^^^^^ hi from good"#,
    );
}

/// Full kumomta `set_meta` style lint: checks first arg of `:set_meta(key)`
/// against an allowlist and the `x_` prefix convention.
#[tokio::test]
async fn kumomta_set_meta_lint() {
    common::assert_plugin_diagnostics!(
        r#"
local lint = require("shingetsu.lint")
lint.declare { name = "kumomta_set_meta", description = "meta key check" }

local KNOWN_META = { queue = true, routing = true }

lint.on("method_call", function(call, ctx)
    if call.method ~= "set_meta" then return end
    local key = call.args[1]
    if not key or key.kind ~= "string_literal" then return end
    local v = key.string_value
    if KNOWN_META[v] then return end
    if v:starts_with("x_") then return end
    ctx:warn(
        key.span,
        `metadata key "{v}" is not pre-defined and may collide with future keys`,
        "prefix the key with 'x_' to avoid collision"
    )
end)
"#,
        r#"msg:set_meta("queue", 1) msg:set_meta("bogus", 2) msg:set_meta("x_my", 3)"#,
        r#"warning[project:kumomta_set_meta]: metadata key "bogus" is not pre-defined and may collide with future keys
 --> test.lua:1:39
  |
1 | msg:set_meta("queue", 1) msg:set_meta("bogus", 2) msg:set_meta("x_my", 3)
  |                                       ^^^^^^^ metadata key "bogus" is not pre-defined and may collide with future keys
  |
help: prefix the key with 'x_' to avoid collision"#,
    );
}

/// kumomta `record_doc_matches_runtime` style lint: parses `@field` tags
/// from a doc comment and warns when the table constructor is missing them.
#[tokio::test]
async fn kumomta_record_doc_matches_runtime_lint() {
    common::assert_plugin_diagnostics!(
        r#"
local lint = require("shingetsu.lint")
lint.declare { name = "kumomta_record_doc_matches", description = "..." }

local function declared_fields(doc)
    local names = {}
    for line in doc:gmatch("[^\n]+") do
        local name = line:match("^@field%s+(%S+)")
        if name then
            names[#names + 1] = name
        end
    end
    return names
end

lint.on("function_call", function(call, ctx)
    local doc = call.doc_comment
    if not doc then return end
    local declared = declared_fields(doc)
    if #declared == 0 then return end
    local tbl = call.args[2]
    if not tbl or tbl.kind ~= "table_constructor" then return end
    local present = {}
    for _, entry in ipairs(tbl.entries) do
        if entry.kind == "named" then
            present[entry.name] = true
        end
    end
    for _, field in ipairs(declared) do
        if not present[field] then
            ctx:warn(
                tbl.span,
                `@field "{field}" is missing from the record table`,
                "add the field to the constructor, or remove the @field tag"
            )
        end
    end
end)
"#,
        r#"--- @class Worker
--- @field name string
local Worker = mod.record("Worker", { naem = "string" })"#,
        concat!(
            r#"warning[project:kumomta_record_doc_matches]: @field "name" is missing from the record table
 --> test.lua:3:37
  |
3 | local Worker = mod.record("Worker", { naem = "string" })
  |                                     "#,
            "^^^^^^^^^^^^^^^^^^^",
            r#" @field "name" is missing from the record table
  |
help: add the field to the constructor, or remove the @field tag"#,
        ),
    );
}

/// `ctx:is_same_line` returns true when both spans start on the same line.
#[tokio::test]
async fn ctx_is_same_line_matches_span_lines() {
    common::assert_plugin_diagnostics!(
        r#"
local lint = require("shingetsu.lint")
lint.declare { name = "same_line", description = "d" }
lint.on("binop", function(expr, ctx)
    local same = ctx:is_same_line(expr.op_span, expr.span)
    if not same then
        ctx:warn(expr.span, "op and binop span should share a line")
    end
end)
"#,
        "local x = 1 + 2",
        "",
    );
}

/// `ctx:constant_value` returns the literal string value for string
/// literal expressions.
#[tokio::test]
async fn ctx_constant_value_returns_literal_value() {
    common::assert_plugin_diagnostics!(
        r#"
local lint = require("shingetsu.lint")
lint.declare { name = "const_val", description = "d" }
lint.on("string_literal", function(expr, ctx)
    local v = ctx:constant_value(expr)
    if v ~= "hello" then
        ctx:warn(expr.span, "expected 'hello' but got: " .. tostring(v))
    end
end)
"#,
        r#"local x = "hello""#,
        "",
    );
}

// ---------------------------------------------------------------------------
// Orchestrator / multi-plugin tests
// ---------------------------------------------------------------------------

/// Two plugins listening on `method_call` each emit their own diagnostic;
/// the orchestrator concatenates them in load order.
#[tokio::test]
async fn lint_chunk_runs_every_plugin() {
    let plugin_a = write_temp_file(
        r#"
local lint = require("shingetsu.lint")
lint.declare { name = "alpha", description = "a" }
lint.on("method_call", function(call, ctx) ctx:warn(call.span, "alpha saw " .. call.method) end)
"#,
    );
    let plugin_b = write_temp_file(
        r#"
local lint = require("shingetsu.lint")
lint.declare { name = "beta", description = "b" }
lint.on("method_call", function(call, ctx) ctx:warn(call.span, "beta saw " .. call.method) end)
"#,
    );
    let loaded = LoadedPlugins::load_from_paths(&[plugin_a.path(), plugin_b.path()])
        .await
        .expect("load");
    k9::assert_equal!(loaded.len(), 2);

    let source_text = "obj:foo()";
    let opts = CompileOptions {
        type_check: true,
        debug_info: true,
        source_name: Arc::new("@test.lua".to_string()),
    };
    let compiler = Compiler::new(opts, GlobalEnv::new().global_type_map());
    let compiled = compiler
        .compile_with_ast(source_text)
        .await
        .expect("compile");
    let lint_ir = compiled.lint_ir.expect("lint_ir");
    let diags = loaded
        .lint_chunk(Arc::new("@test.lua".to_string()), &lint_ir)
        .await
        .expect("dispatch");

    let rendered = render_warnings(&diags, source_text, RenderStyle::Plain);
    k9::assert_equal!(
        rendered,
        r#"warning[project:alpha]: alpha saw foo
 --> test.lua:1:1
  |
1 | obj:foo()
  | ^^^^^^^^ alpha saw foo
warning[project:beta]: beta saw foo
 --> test.lua:1:1
  |
1 | obj:foo()
  | ^^^^^^^^ beta saw foo"#
    );
}

/// Two plugins declaring the same name are caught by the orchestrator.
#[tokio::test]
async fn duplicate_name_across_plugins_errors() {
    let plugin_a = write_temp_file(
        r#"
local lint = require("shingetsu.lint")
lint.declare { name = "shared", description = "first" }
"#,
    );
    let plugin_b = write_temp_file(
        r#"
local lint = require("shingetsu.lint")
lint.declare { name = "shared", description = "second" }
"#,
    );
    let err = LoadedPlugins::load_from_paths(&[plugin_a.path(), plugin_b.path()])
        .await
        .expect_err("should fail");
    let err = err
        .replace(plugin_a.path().to_str().expect("utf8"), "<plugin_a>")
        .replace(plugin_b.path().to_str().expect("utf8"), "<plugin_b>");
    k9::assert_equal!(
        err,
        concat!(
            r#"error[project:plugin_loader]: lint plugin 'shared' is declared more than once
 --> <plugin_b>:3:1
  |
3 | lint.declare { name = "shared", description = "second" }
  | ^^^^^^^^^^^^ this declaration conflicts
  |
 ::: <plugin_a>:3:1
  |
3 | lint.declare { name = "shared", description = "first" }
  | ------------ first declared here
  |
help: each plugin file must declare a unique name; rename one of the conflicting plugins"#,
        )
    );
}
