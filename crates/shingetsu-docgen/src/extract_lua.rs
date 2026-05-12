//! Build a [`DocModel`] from pure-Lua "library" source files.
//!
//! Used by `shingetsu doc extract-lua` so embedder Lua modules
//! (kumomta's policy-extras helpers, wezterm's lua-side utilities)
//! can be documented and type-checked alongside the Rust-defined
//! surface.
//!
//! # Pipeline
//!
//! Each file flows through:
//! 1. `Compiler::compile` -- the compiler's lowering pass walks
//!    the AST, infers the module-table shape, and harvests preceding
//!    `---` doc-comment text onto `TableField.doc`.
//! 2. AST walk to harvest `---` doc-comment blocks and bind them to
//!    field names.
//! 3. Iterate the compiler's typed table fields, pairing each with
//!    its harvested doc block, applying EmmyLua-style tag overrides
//!    (`@param`, `@return`, `@deprecated`, `@nodiscard`, `@hidden`).
//!
//! Supported module shape: `local mod = {} ... return mod`.  Inline
//! `return { ... }` returns and non-table returns produce a warning
//! and an empty module (see the gap note in `notes/LINT.md`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use shingetsu::compiler::{CompileOptions, Compiler, Diagnostic, LintId, Severity, SourceLocation};
use shingetsu::types::{FunctionLuaType, LuaType, TableLuaType};
use shingetsu::GlobalTypeMap;

use crate::{
    DocModel, FieldDoc, FieldDocKind, FunctionDoc, ModuleDoc, ParamDoc, ReturnDoc, TypeRef,
    SCHEMA_VERSION,
};

/// Options for [`extract_from_sources`].
#[derive(Debug, Clone, Default)]
pub struct ExtractOptions {
    /// Root directory used to derive module names.  When `Some`,
    /// the module name for `<root>/foo/bar.lua` is `foo.bar`.  When
    /// `None`, the file's basename (without `.lua`) is used.
    pub root: Option<PathBuf>,
}

/// Errors returned by [`extract_from_sources`].
#[derive(Debug)]
pub enum ExtractError {
    /// Failed to read a source file.
    Io { path: PathBuf, err: std::io::Error },
    /// Parser refused the source file.
    Parse { path: PathBuf, message: String },
    /// Source file is outside `--root`.
    OutsideRoot { path: PathBuf, root: PathBuf },
}

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtractError::Io { path, err } => write!(f, "reading {}: {err}", path.display()),
            ExtractError::Parse { path, message } => {
                write!(f, "parsing {}: {message}", path.display())
            }
            ExtractError::OutsideRoot { path, root } => write!(
                f,
                "source file {} is not under --root {}",
                path.display(),
                root.display()
            ),
        }
    }
}

impl std::error::Error for ExtractError {}

/// One file's contribution to the [`DocModel`] plus the source
/// text that produced it.  The source is returned so the caller
/// can render any associated diagnostics through the standard
/// annotate-snippets pipeline without re-reading the file.
pub struct ExtractedFile {
    pub path: PathBuf,
    pub source: String,
    pub module: ModuleDoc,
    pub diagnostics: Vec<Diagnostic>,
}

/// Extract a [`DocModel`] from every Lua file in `paths`.  Each
/// file produces one [`ModuleDoc`].  Non-conforming module shapes
/// emit a `module_shape` [`Diagnostic`] (severity Warning) on the
/// corresponding [`ExtractedFile`].
pub async fn extract_from_sources(
    paths: &[PathBuf],
    opts: &ExtractOptions,
) -> Result<(DocModel, Vec<ExtractedFile>), ExtractError> {
    let mut files = Vec::with_capacity(paths.len());
    for p in paths {
        files.push(extract_file(p, opts).await?);
    }
    let mut modules: Vec<ModuleDoc> = files.iter().map(|f| f.module.clone()).collect();
    modules.sort_by(|a, b| a.name.cmp(&b.name));
    Ok((
        DocModel {
            schema_version: SCHEMA_VERSION,
            modules,
            userdata_types: vec![],
            globals: vec![],
            events: vec![],
        },
        files,
    ))
}

async fn extract_file(path: &Path, opts: &ExtractOptions) -> Result<ExtractedFile, ExtractError> {
    let src = std::fs::read_to_string(path).map_err(|err| ExtractError::Io {
        path: path.to_path_buf(),
        err,
    })?;
    let module_name = derive_module_name(path, opts.root.as_deref())?;
    let source_name = Arc::new(format!("@{}", path.display()));

    let compile_opts = CompileOptions {
        debug_info: false,
        source_name: Arc::clone(&source_name),
        type_check: false,
    };
    let compiler = Compiler::new(compile_opts, GlobalTypeMap::default());
    let bytecode = compiler
        .compile(&src)
        .await
        .map_err(|e| ExtractError::Parse {
            path: path.to_path_buf(),
            message: format!("{e:?}"),
        })?;

    let module = ModuleDoc {
        name: module_name,
        doc: None,
        strict: false,
        fields: vec![],
        functions: vec![],
        partial: false,
    };
    let mut file = ExtractedFile {
        path: path.to_path_buf(),
        source: src,
        module,
        diagnostics: Vec::new(),
    };

    let Some(table) = module_table_from_return_type(&bytecode.module_type_info.return_type) else {
        file.diagnostics
            .push(module_shape_diagnostic(&bytecode, &source_name));
        return Ok(file);
    };

    for field in &table.fields {
        let name = bytes_to_string(field.name.as_ref());
        let doc_block = field.doc.as_deref().map(parse_doc_text);
        let annots = doc_block.as_ref().map(|d| &d.annotations);
        if annots.map(|a| a.hidden).unwrap_or(false) {
            continue;
        }
        match &field.lua_type {
            LuaType::Function(f) => {
                file.module
                    .functions
                    .push(build_function_doc(&name, f, doc_block.as_ref()));
            }
            other => {
                file.module
                    .fields
                    .push(build_field_doc(&name, other, doc_block.as_ref()));
            }
        }
    }

    Ok(file)
}

/// Build a `module_shape` warning diagnostic.  When the chunk has
/// an explicit `return`, the carat sits on that statement and the
/// message describes why the returned value isn't usable as a
/// module.  When the chunk has no `return`, the carat sits on the
/// last top-level statement and the message advises adding one.
fn module_shape_diagnostic(
    bytecode: &shingetsu::compiler::Bytecode,
    source_name: &Arc<String>,
) -> Diagnostic {
    let info = &bytecode.module_type_info;
    let location = info
        .return_location
        .clone()
        .map(SourceLocation::from)
        .unwrap_or_else(|| SourceLocation {
            source_name: Arc::clone(source_name),
            line: 0,
            column: 0,
            byte_offset: 0,
            byte_len: 0,
        });
    let (message, help) = if info.has_explicit_return {
        (
            "module return type could not be inferred as a table; \
             expected `local mod = {} ... return mod`"
                .to_string(),
            "shingetsu doc extract-lua only supports the canonical \
             `local mod = {} ... return mod` shape"
                .to_string(),
        )
    } else {
        (
            "file has no `return` statement; nothing to extract as a module".to_string(),
            "add `return <module-table>` at the end of the file to make a \
             proper module"
                .to_string(),
        )
    };
    Diagnostic {
        lint: LintId::ModuleShape,
        severity: Severity::Warning,
        location,
        message,
        help: Some(help),
        primary_label: None,
        secondary_spans: vec![],
    }
}

/// Treat both `LuaType::Module` and `LuaType::Table` as a table for
/// the purposes of field iteration.  Returns `None` for other shapes.
fn module_table_from_return_type(ty: &Option<LuaType>) -> Option<&TableLuaType> {
    match ty {
        Some(LuaType::Table(t)) => Some(t),
        Some(LuaType::Module(_)) => None,
        _ => None,
    }
}

fn build_function_doc(name: &str, f: &FunctionLuaType, doc: Option<&DocComment>) -> FunctionDoc {
    let annots = doc.map(|d| &d.annotations);
    let summary = doc.and_then(|d| {
        if d.summary.is_empty() {
            None
        } else {
            Some(d.summary.clone())
        }
    });
    let is_method = f.is_method;

    // Start with the compiler's params (skipping the implicit self
    // for methods, per FunctionLuaType convention).
    let skip = if is_method { 1 } else { 0 };
    let mut params: Vec<ParamDoc> = f
        .params
        .iter()
        .skip(skip)
        .map(|p| {
            let (ty, optional) = match &p.lua_type {
                LuaType::Optional(inner) => (TypeRef::from_lua_type(inner), true),
                other => (TypeRef::from_lua_type(other), false),
            };
            ParamDoc {
                name: p.name.as_ref().map(|n| bytes_to_string(n.as_ref())),
                ty,
                optional,
                doc: None,
            }
        })
        .collect();

    let mut returns: Vec<ReturnDoc> = f
        .returns
        .iter()
        .map(|r| ReturnDoc {
            ty: TypeRef::from_lua_type(r),
            doc: None,
        })
        .collect();

    // Apply EmmyLua-tag overrides on top of compiler inference.
    if let Some(annots) = annots {
        for pa in &annots.params {
            if let Some(p) = params
                .iter_mut()
                .find(|p| p.name.as_deref() == Some(&pa.name))
            {
                if let Some(ty) = parse_type_ref(&pa.ty) {
                    p.ty = ty;
                }
                if !pa.desc.is_empty() {
                    p.doc = Some(pa.desc.clone());
                }
            }
        }
        if !annots.returns.is_empty() {
            returns = annots
                .returns
                .iter()
                .map(|r| ReturnDoc {
                    ty: parse_type_ref(&r.ty).unwrap_or(TypeRef::Any),
                    doc: if r.desc.is_empty() {
                        None
                    } else {
                        Some(r.desc.clone())
                    },
                })
                .collect();
        }
    }

    let variadic = f.variadic.as_deref().map(TypeRef::from_lua_type);

    let synopsis =
        crate::synopsis::render_synopsis("", name, &params, variadic.as_ref(), &returns, is_method);

    FunctionDoc {
        name: name.to_string(),
        doc: summary,
        synopsis,
        params,
        variadic,
        variadic_doc: None,
        returns,
        is_method,
        examples: vec![],
        deprecated: annots.and_then(|a| a.deprecated.clone()),
        must_use: annots.and_then(|a| a.must_use.clone()),
    }
}

fn build_field_doc(name: &str, ty: &LuaType, doc: Option<&DocComment>) -> FieldDoc {
    let annots = doc.map(|d| &d.annotations);
    let summary = doc.and_then(|d| {
        if d.summary.is_empty() {
            None
        } else {
            Some(d.summary.clone())
        }
    });
    FieldDoc {
        name: name.to_string(),
        doc: summary,
        ty: TypeRef::from_lua_type(ty),
        kind: FieldDocKind::ReadWrite,
        examples: vec![],
        deprecated: annots.and_then(|a| a.deprecated.clone()),
    }
}

/// Derive a module name from a path + optional root.
fn derive_module_name(path: &Path, root: Option<&Path>) -> Result<String, ExtractError> {
    let stem_path = path.with_extension("");
    let relative = if let Some(root) = root {
        match stem_path.strip_prefix(root) {
            Ok(rel) => rel.to_path_buf(),
            Err(_) => {
                return Err(ExtractError::OutsideRoot {
                    path: path.to_path_buf(),
                    root: root.to_path_buf(),
                });
            }
        }
    } else {
        stem_path
            .file_name()
            .map(PathBuf::from)
            .unwrap_or_else(|| stem_path.clone())
    };
    let name = relative
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(".");
    Ok(name)
}

// ---------------------------------------------------------------------------
// Doc comment harvesting
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct DocComment {
    summary: String,
    annotations: DocAnnotations,
}

#[derive(Debug, Clone, Default)]
struct DocAnnotations {
    deprecated: Option<String>,
    must_use: Option<String>,
    hidden: bool,
    params: Vec<ParamAnnotation>,
    returns: Vec<ReturnAnnotation>,
}

#[derive(Debug, Clone)]
struct ParamAnnotation {
    name: String,
    ty: String,
    desc: String,
}

#[derive(Debug, Clone)]
struct ReturnAnnotation {
    ty: String,
    desc: String,
}

/// Parse the raw doc-comment text the compiler captured on
/// `TableField.doc` into a structured summary + EmmyLua tag set.
fn parse_doc_text(text: &str) -> DocComment {
    let lines: Vec<String> = text.split('\n').map(|l| l.to_string()).collect();
    parse_doc_block(&lines)
}

fn parse_doc_block(lines: &[String]) -> DocComment {
    let mut summary_lines: Vec<String> = Vec::new();
    let mut annots = DocAnnotations::default();
    for line in lines {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix('@') {
            let (tag, body) = split_tag(rest);
            apply_tag(&mut annots, tag, body);
        } else if annots.params.is_empty()
            && annots.returns.is_empty()
            && annots.deprecated.is_none()
            && annots.must_use.is_none()
            && !annots.hidden
        {
            summary_lines.push(line.clone());
        } else {
            extend_last_tag_desc(&mut annots, line);
        }
    }
    DocComment {
        summary: summary_lines.join("\n").trim_end().to_string(),
        annotations: annots,
    }
}

fn split_tag(rest: &str) -> (&str, &str) {
    match rest.find(|c: char| c.is_whitespace()) {
        Some(idx) => (&rest[..idx], rest[idx..].trim_start()),
        None => (rest, ""),
    }
}

fn apply_tag(annots: &mut DocAnnotations, tag: &str, body: &str) {
    match tag {
        "deprecated" => {
            annots.deprecated = Some(body.to_string());
        }
        "nodiscard" => {
            annots.must_use = Some(body.to_string());
        }
        "hidden" => {
            annots.hidden = true;
        }
        "param" => {
            let (name, rest) = split_tag(body);
            let (ty, desc) = split_tag(rest);
            annots.params.push(ParamAnnotation {
                name: name.to_string(),
                ty: ty.to_string(),
                desc: desc.to_string(),
            });
        }
        "return" => {
            let (ty, desc) = split_tag(body);
            annots.returns.push(ReturnAnnotation {
                ty: ty.to_string(),
                desc: desc.to_string(),
            });
        }
        _ => {
            // Unknown tag: ignored.
        }
    }
}

fn extend_last_tag_desc(annots: &mut DocAnnotations, line: &str) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    if let Some(last) = annots.params.last_mut() {
        push_desc_line(&mut last.desc, trimmed);
        return;
    }
    if let Some(last) = annots.returns.last_mut() {
        push_desc_line(&mut last.desc, trimmed);
        return;
    }
    if let Some(d) = annots.deprecated.as_mut() {
        push_desc_line(d, trimmed);
        return;
    }
    if let Some(d) = annots.must_use.as_mut() {
        push_desc_line(d, trimmed);
    }
}

fn push_desc_line(dest: &mut String, line: &str) {
    if !dest.is_empty() {
        dest.push('\n');
    }
    dest.push_str(line);
}

fn parse_type_ref(s: &str) -> Option<TypeRef> {
    if s.is_empty() {
        return None;
    }
    Some(match s {
        "any" => TypeRef::Any,
        "nil" => TypeRef::Nil,
        "boolean" => TypeRef::Boolean,
        "number" => TypeRef::Number,
        "integer" => TypeRef::Integer,
        "float" => TypeRef::Float,
        "string" => TypeRef::String,
        other => TypeRef::Named {
            name: other.to_string(),
        },
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn bytes_to_string(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FieldDocKind, ParamDoc, ReturnDoc};

    async fn extract_str(name: &str, source: &str) -> Result<ExtractedFile, ExtractError> {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join(format!("{name}.lua"));
        std::fs::write(&path, source).expect("write");
        let opts = ExtractOptions {
            root: Some(tmp.path().to_path_buf()),
        };
        extract_file(&path, &opts).await
    }

    /// Render an [`ExtractedFile`]'s diagnostics in plain (no-ANSI)
    /// style with the file path normalised so assertions match across
    /// the tempfile-generated path.
    fn render_diags(file: &ExtractedFile) -> String {
        let rendered = shingetsu::diagnostic::render_warnings(
            &file.diagnostics,
            &file.source,
            shingetsu::diagnostic::RenderStyle::Plain,
        );
        rendered.replace(file.path.to_str().expect("non-utf8"), "<FILE>")
    }

    #[tokio::test]
    async fn local_table_with_function() {
        let src = "\
local mod = {}

--- Configure the queue from a TOML file.
function mod.configure(path: string): boolean
    return true
end

return mod
";
        let file = extract_str("queue_helper", src).await.expect("extract");
        let expected = ModuleDoc {
            name: "queue_helper".to_string(),
            doc: None,
            strict: false,
            fields: vec![],
            functions: vec![FunctionDoc {
                name: "configure".to_string(),
                doc: Some("Configure the queue from a TOML file.".to_string()),
                synopsis: "configure(path) -> boolean".to_string(),
                params: vec![ParamDoc {
                    name: Some("path".to_string()),
                    ty: TypeRef::String,
                    optional: false,
                    doc: None,
                }],
                variadic: None,
                variadic_doc: None,
                returns: vec![ReturnDoc {
                    ty: TypeRef::Boolean,
                    doc: None,
                }],
                is_method: false,
                examples: vec![],
                deprecated: None,
                must_use: None,
            }],
            partial: false,
        };
        k9::assert_equal!(file.module, expected);
        k9::assert_equal!(render_diags(&file), "");
    }

    #[tokio::test]
    async fn emmylua_param_overrides_annotation() {
        let src = "\
local mod = {}

--- Compute something.
--- @param x integer  override description
function mod.compute(x: number): boolean
    return true
end

return mod
";
        let file = extract_str("foo", src).await.expect("extract");
        let expected = ModuleDoc {
            name: "foo".to_string(),
            doc: None,
            strict: false,
            fields: vec![],
            functions: vec![FunctionDoc {
                name: "compute".to_string(),
                doc: Some("Compute something.".to_string()),
                synopsis: "compute(x) -> boolean".to_string(),
                params: vec![ParamDoc {
                    name: Some("x".to_string()),
                    ty: TypeRef::Integer,
                    optional: false,
                    doc: Some("override description".to_string()),
                }],
                variadic: None,
                variadic_doc: None,
                returns: vec![ReturnDoc {
                    ty: TypeRef::Boolean,
                    doc: None,
                }],
                is_method: false,
                examples: vec![],
                deprecated: None,
                must_use: None,
            }],
            partial: false,
        };
        k9::assert_equal!(file.module, expected);
        k9::assert_equal!(render_diags(&file), "");
    }

    #[tokio::test]
    async fn deprecated_and_nodiscard() {
        let src = "\
local mod = {}

--- Old way of doing things.
--- @deprecated use `new_func` instead
function mod.old_func() end

--- Critical hash; don't drop the result.
--- @nodiscard the hash is what you wanted
function mod.message_hash() end

return mod
";
        let file = extract_str("foo", src).await.expect("extract");
        let expected_functions = vec![
            FunctionDoc {
                name: "old_func".to_string(),
                doc: Some("Old way of doing things.".to_string()),
                synopsis: "old_func()".to_string(),
                params: vec![],
                variadic: None,
                variadic_doc: None,
                returns: vec![],
                is_method: false,
                examples: vec![],
                deprecated: Some("use `new_func` instead".to_string()),
                must_use: None,
            },
            FunctionDoc {
                name: "message_hash".to_string(),
                doc: Some("Critical hash; don't drop the result.".to_string()),
                synopsis: "message_hash()".to_string(),
                params: vec![],
                variadic: None,
                variadic_doc: None,
                returns: vec![],
                is_method: false,
                examples: vec![],
                deprecated: None,
                must_use: Some("the hash is what you wanted".to_string()),
            },
        ];
        k9::assert_equal!(file.module.functions, expected_functions);
        k9::assert_equal!(render_diags(&file), "");
    }

    #[tokio::test]
    async fn hidden_function_omitted() {
        let src = "\
local mod = {}

--- Internal helper.
--- @hidden
function mod.internal() end

function mod.public() end

return mod
";
        let file = extract_str("foo", src).await.expect("extract");
        let expected_functions = vec![FunctionDoc {
            name: "public".to_string(),
            doc: None,
            synopsis: "public()".to_string(),
            params: vec![],
            variadic: None,
            variadic_doc: None,
            returns: vec![],
            is_method: false,
            examples: vec![],
            deprecated: None,
            must_use: None,
        }];
        k9::assert_equal!(file.module.functions, expected_functions);
        k9::assert_equal!(render_diags(&file), "");
    }

    #[tokio::test]
    async fn field_assignment_tracked() {
        // The kumomta `mod.bar = expr` shape: the compiler surfaces
        // these as entries on the module type.  `helper` resolves to
        // a function-typed local so lands in `functions`; `something`
        // resolves to `Any` (the compiler can't yet infer call-result
        // types) so lands in `fields`.
        let src = "\
local mod = {}

local helper = function(x) return x end

--- A function-typed re-export.
mod.helper = helper

--- A field that resolves to Any (call-result, not yet inferable).
mod.something = some_call()

return mod
";
        // Both assignments land in `fields` with `TypeRef::Any`:
        // `mod.helper = helper` where `helper` is a `local fn =
        // function() end` -- the compiler does not yet flow the
        // function literal's type onto the local, so `infer_expr_type`
        // falls back to Any.  `mod.something = some_call()` falls
        // back to Any because call-result inference is the other
        // Phase 3b follow-up gap.  Documented in notes/LINT.md.
        let file = extract_str("foo", src).await.expect("extract");
        let expected_module = ModuleDoc {
            name: "foo".to_string(),
            doc: None,
            strict: false,
            fields: vec![
                FieldDoc {
                    name: "helper".to_string(),
                    doc: Some("A function-typed re-export.".to_string()),
                    ty: TypeRef::Any,
                    kind: FieldDocKind::ReadWrite,
                    examples: vec![],
                    deprecated: None,
                },
                FieldDoc {
                    name: "something".to_string(),
                    doc: Some(
                        "A field that resolves to Any (call-result, not yet inferable)."
                            .to_string(),
                    ),
                    ty: TypeRef::Any,
                    kind: FieldDocKind::ReadWrite,
                    examples: vec![],
                    deprecated: None,
                },
            ],
            functions: vec![],
            partial: false,
        };
        k9::assert_equal!(file.module, expected_module);
        k9::assert_equal!(render_diags(&file), "");
    }

    #[tokio::test]
    async fn warns_on_non_table_return() {
        let src = "return 42\n";
        let file = extract_str("foo", src).await.expect("extract");
        let expected_module = ModuleDoc {
            name: "foo".to_string(),
            doc: None,
            strict: false,
            fields: vec![],
            functions: vec![],
            partial: false,
        };
        k9::assert_equal!(file.module, expected_module);
        k9::assert_equal!(
            render_diags(&file),
            "warning[module_shape]: module return type could not be inferred as a table; expected `local mod = {} ... return mod`
 --> <FILE>:1:1
  |
1 | return 42
  | ^^^^^^^^^ module return type could not be inferred as a table; expected `local mod = {} ... return mod`
  |
help: shingetsu doc extract-lua only supports the canonical `local mod = {} ... return mod` shape"
        );
    }

    #[tokio::test]
    async fn warns_on_no_return() {
        let src = "local x = 1\n";
        let file = extract_str("foo", src).await.expect("extract");
        k9::assert_equal!(
            render_diags(&file),
            "warning[module_shape]: file has no `return` statement; nothing to extract as a module
 --> <FILE>:1:1
  |
1 | local x = 1
  | ^^^^^^^^^^^ file has no `return` statement; nothing to extract as a module
  |
help: add `return <module-table>` at the end of the file to make a proper module"
        );
    }

    #[tokio::test]
    async fn module_name_from_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("policy-extras");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("queue.lua");
        std::fs::write(&path, "local mod = {}\nreturn mod\n").expect("write");
        let opts = ExtractOptions {
            root: Some(tmp.path().to_path_buf()),
        };
        let file = extract_file(&path, &opts).await.expect("extract");
        k9::assert_equal!(
            file.module,
            ModuleDoc {
                name: "policy-extras.queue".to_string(),
                doc: None,
                strict: false,
                fields: vec![],
                functions: vec![],
                partial: false,
            }
        );
        k9::assert_equal!(render_diags(&file), "");
    }

    #[tokio::test]
    async fn module_name_no_root() {
        let opts = ExtractOptions { root: None };
        let name = derive_module_name(Path::new("foo/bar/baz.lua"), opts.root.as_deref()).unwrap();
        k9::assert_equal!(name, "baz");
    }
}
