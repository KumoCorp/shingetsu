//! Markdown emitter for [`DocModel`].
//!
//! [`render_markdown`] produces a self-contained subtree of markdown
//! pages: one for each module and userdata type, plus an index page.
//! All internal links are relative within the subtree, so the entire
//! output can be mounted under any path in a larger authored site
//! without rewriting links.

use std::collections::HashMap;
use std::fmt::Write;
use std::path::PathBuf;

use crate::display::display;
use crate::{
    DocModel, FieldDoc, FieldDocKind, FunctionDoc, MetamethodDoc, ModuleDoc, ParamDoc, ReturnDoc,
    TypeRef, UserdataDoc,
};

/// Options controlling markdown output layout and styling.
#[derive(Clone, Debug)]
pub struct MdOptions {
    /// Front-matter style for every emitted page.
    pub front_matter: FrontMatterStyle,
    /// If a module or userdata type has more than this many addressable
    /// items (fields + functions/methods + metamethods) it is split
    /// into one page per item with a parent index page.  Set to
    /// `usize::MAX` to always inline; `0` to always split.
    pub split_threshold: usize,
    /// Per-name override map. A name present here overrides
    /// `split_threshold` for that module or userdata type.
    pub split_overrides: HashMap<String, SplitMode>,
    /// Optional URL prefix prepended to all generated *outgoing* links
    /// (cross-links between generated pages).  Useful for sites that
    /// mount the generated subtree under a non-root URL.  Internal
    /// links remain valid without this prefix; setting it is purely
    /// for consumers that prefer absolute URLs.
    pub link_prefix: Option<String>,
}

impl Default for MdOptions {
    fn default() -> Self {
        Self {
            front_matter: FrontMatterStyle::None,
            split_threshold: 12,
            split_overrides: HashMap::new(),
            link_prefix: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitMode {
    Inline,
    Split,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrontMatterStyle {
    None,
    /// Emit a YAML block compatible with Zensical / MkDocs.
    Zensical,
    MkDocs,
    /// Emit a YAML block compatible with Hugo.
    Hugo,
}

/// One markdown file in the emitted output.  `path` is relative to the
/// caller-chosen output root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MdFile {
    pub path: PathBuf,
    pub content: String,
}

/// Render a [`DocModel`] as a self-contained subtree of markdown
/// files.
///
/// Output layout:
/// - `index.md` — top-level index page listing modules and userdata
///   types.
/// - `modules/<name>/index.md` — per-module page (inline or split
///   parent).  In split mode each function/field also gets its own
///   `modules/<name>/<item>.md` page.
/// - `types/<name>/index.md` — per-userdata-type page, mirroring
///   the module layout.
///
/// Files are returned in deterministic order (index first, then
/// modules sorted by name, then types sorted by name, with split
/// item pages immediately after each parent).
pub fn render_markdown(model: &DocModel, opts: &MdOptions) -> Vec<MdFile> {
    // First pass: decide split vs inline for every module and type.
    // Stored in a layout map so cross-linking knows which form to
    // target.
    let layout = Layout::compute(model, opts);

    let mut files: Vec<MdFile> = Vec::new();

    files.push(MdFile {
        path: PathBuf::from("index.md"),
        content: render_index(model, opts),
    });

    for m in &model.modules {
        let mode = layout.module_mode(&m.name);
        let parent_path = format!("modules/{}/index.md", m.name);
        files.push(MdFile {
            path: PathBuf::from(&parent_path),
            content: render_module_parent(m, mode, opts, &layout),
        });
        if mode == SplitMode::Split {
            for fld in &m.fields {
                files.push(MdFile {
                    path: PathBuf::from(format!("modules/{}/{}.md", m.name, fld.name)),
                    content: render_field_page(&m.name, "module", fld, opts, &layout),
                });
            }
            for func in &m.functions {
                files.push(MdFile {
                    path: PathBuf::from(format!("modules/{}/{}.md", m.name, func.name)),
                    content: render_function_page(&m.name, "module", func, opts, &layout),
                });
            }
        }
    }

    for ud in &model.userdata_types {
        let mode = layout.userdata_mode(&ud.name);
        files.push(MdFile {
            path: PathBuf::from(format!("types/{}/index.md", ud.name)),
            content: render_userdata_parent(ud, mode, opts, &layout),
        });
        if mode == SplitMode::Split {
            for fld in &ud.fields {
                files.push(MdFile {
                    path: PathBuf::from(format!("types/{}/{}.md", ud.name, fld.name)),
                    content: render_field_page(&ud.name, "type", fld, opts, &layout),
                });
            }
            for m in &ud.methods {
                files.push(MdFile {
                    path: PathBuf::from(format!("types/{}/{}.md", ud.name, m.name)),
                    content: render_function_page(&ud.name, "type", m, opts, &layout),
                });
            }
            for mm in &ud.metamethods {
                files.push(MdFile {
                    path: PathBuf::from(format!("types/{}/{}.md", ud.name, mm.method)),
                    content: render_metamethod_page(&ud.name, mm, opts, &layout),
                });
            }
        }
    }

    files
}

/// Cached split-vs-inline decisions for every module and userdata
/// type, so cross-link emission can target the correct URL form
/// (anchor on parent vs separate page).
struct Layout {
    modules: HashMap<String, SplitMode>,
    userdata: HashMap<String, SplitMode>,
}

impl Layout {
    fn compute(model: &DocModel, opts: &MdOptions) -> Self {
        let mut modules = HashMap::new();
        for m in &model.modules {
            let count = m.fields.len() + m.functions.len();
            modules.insert(m.name.clone(), pick_mode(&m.name, count, opts));
        }
        let mut userdata = HashMap::new();
        for ud in &model.userdata_types {
            let count = ud.fields.len() + ud.methods.len() + ud.metamethods.len();
            userdata.insert(ud.name.clone(), pick_mode(&ud.name, count, opts));
        }
        Layout { modules, userdata }
    }

    fn module_mode(&self, name: &str) -> SplitMode {
        self.modules.get(name).copied().unwrap_or(SplitMode::Inline)
    }

    fn userdata_mode(&self, name: &str) -> SplitMode {
        self.userdata
            .get(name)
            .copied()
            .unwrap_or(SplitMode::Inline)
    }
}

fn pick_mode(name: &str, item_count: usize, opts: &MdOptions) -> SplitMode {
    if let Some(o) = opts.split_overrides.get(name) {
        return *o;
    }
    if item_count > opts.split_threshold {
        SplitMode::Split
    } else {
        SplitMode::Inline
    }
}

// ---------------------------------------------------------------------------
// Page renderers
// ---------------------------------------------------------------------------

fn render_index(model: &DocModel, opts: &MdOptions) -> String {
    let mut out = String::new();
    push_front_matter(&mut out, opts.front_matter, "Reference");
    out.push_str("# Reference\n\n");

    if !model.modules.is_empty() {
        out.push_str("## Modules\n\n");
        for m in &model.modules {
            writeln!(
                out,
                "- [`{}`]({}) — {}",
                m.name,
                module_link("", &m.name, opts),
                m.doc.as_deref().unwrap_or("").trim()
            )
            .ok();
        }
        out.push('\n');
    }

    if !model.userdata_types.is_empty() {
        out.push_str("## Types\n\n");
        for ud in &model.userdata_types {
            writeln!(
                out,
                "- [`{}`]({}) — {}",
                ud.name,
                userdata_link("", &ud.name, opts),
                ud.doc.as_deref().unwrap_or("").trim()
            )
            .ok();
        }
        out.push('\n');
    }

    out
}

fn render_module_parent(
    m: &ModuleDoc,
    mode: SplitMode,
    opts: &MdOptions,
    layout: &Layout,
) -> String {
    let from_dir = format!("modules/{}/", m.name);
    let mut out = String::new();
    push_front_matter(&mut out, opts.front_matter, &m.name);
    writeln!(out, "# {}\n", m.name).ok();
    if let Some(doc) = &m.doc {
        out.push_str(doc);
        out.push_str("\n\n");
    }

    match mode {
        SplitMode::Inline => {
            if !m.fields.is_empty() {
                out.push_str("## Fields\n\n");
                for f in &m.fields {
                    render_field_inline(&mut out, f, &from_dir, opts, layout);
                }
            }
            if !m.functions.is_empty() {
                out.push_str("## Functions\n\n");
                for func in &m.functions {
                    render_function_inline(&mut out, func, &from_dir, opts, layout);
                }
            }
        }
        SplitMode::Split => {
            if !m.fields.is_empty() {
                out.push_str("## Fields\n\n");
                for f in &m.fields {
                    writeln!(out, "- [`{}`]({}.md)", f.name, f.name).ok();
                }
                out.push('\n');
            }
            if !m.functions.is_empty() {
                out.push_str("## Functions\n\n");
                for func in &m.functions {
                    writeln!(
                        out,
                        "- [`{}`]({}.md) — {}",
                        func.synopsis,
                        func.name,
                        func.doc.as_deref().unwrap_or("").trim()
                    )
                    .ok();
                }
                out.push('\n');
            }
        }
    }

    out
}

fn render_userdata_parent(
    ud: &UserdataDoc,
    mode: SplitMode,
    opts: &MdOptions,
    layout: &Layout,
) -> String {
    let from_dir = format!("types/{}/", ud.name);
    let mut out = String::new();
    push_front_matter(&mut out, opts.front_matter, &ud.name);
    writeln!(out, "# {}\n", ud.name).ok();
    if let Some(doc) = &ud.doc {
        out.push_str(doc);
        out.push_str("\n\n");
    }

    match mode {
        SplitMode::Inline => {
            if !ud.fields.is_empty() {
                out.push_str("## Fields\n\n");
                for f in &ud.fields {
                    render_field_inline(&mut out, f, &from_dir, opts, layout);
                }
            }
            if !ud.methods.is_empty() {
                out.push_str("## Methods\n\n");
                for m in &ud.methods {
                    render_function_inline(&mut out, m, &from_dir, opts, layout);
                }
            }
            if !ud.metamethods.is_empty() {
                out.push_str("## Metamethods\n\n");
                for mm in &ud.metamethods {
                    render_metamethod_inline(&mut out, mm, &from_dir, opts, layout);
                }
            }
        }
        SplitMode::Split => {
            if !ud.fields.is_empty() {
                out.push_str("## Fields\n\n");
                for f in &ud.fields {
                    writeln!(out, "- [`{}`]({}.md)", f.name, f.name).ok();
                }
                out.push('\n');
            }
            if !ud.methods.is_empty() {
                out.push_str("## Methods\n\n");
                for m in &ud.methods {
                    writeln!(
                        out,
                        "- [`{}`]({}.md) — {}",
                        m.synopsis,
                        m.name,
                        m.doc.as_deref().unwrap_or("").trim()
                    )
                    .ok();
                }
                out.push('\n');
            }
            if !ud.metamethods.is_empty() {
                out.push_str("## Metamethods\n\n");
                for mm in &ud.metamethods {
                    writeln!(
                        out,
                        "- [`{}`]({}.md) — {}",
                        mm.synopsis,
                        mm.method,
                        mm.doc.as_deref().unwrap_or("").trim()
                    )
                    .ok();
                }
                out.push('\n');
            }
        }
    }

    out
}

fn render_field_page(
    parent: &str,
    parent_kind: &str,
    f: &FieldDoc,
    opts: &MdOptions,
    layout: &Layout,
) -> String {
    let from_dir = item_page_from_dir(parent_kind, parent);
    let mut out = String::new();
    push_front_matter(&mut out, opts.front_matter, &format!("{parent}.{}", f.name));
    writeln!(out, "# {}.{}\n", parent, f.name).ok();
    render_field_body(&mut out, f, &from_dir, opts, layout);
    out
}

fn render_function_page(
    parent: &str,
    parent_kind: &str,
    func: &FunctionDoc,
    opts: &MdOptions,
    layout: &Layout,
) -> String {
    let from_dir = item_page_from_dir(parent_kind, parent);
    let mut out = String::new();
    push_front_matter(&mut out, opts.front_matter, &func.synopsis);
    writeln!(out, "# {}\n", func.synopsis_anchor_title(parent)).ok();
    writeln!(out, "```\n{}\n```\n", func.synopsis).ok();
    render_function_body(&mut out, func, &from_dir, opts, layout);
    out
}

fn render_metamethod_page(
    parent: &str,
    mm: &MetamethodDoc,
    opts: &MdOptions,
    layout: &Layout,
) -> String {
    let from_dir = item_page_from_dir("type", parent);
    let mut out = String::new();
    push_front_matter(
        &mut out,
        opts.front_matter,
        &format!("{parent}.{}", mm.method),
    );
    writeln!(out, "# {parent}.{}\n", mm.method).ok();
    writeln!(out, "```\n{}\n```\n", mm.synopsis).ok();
    render_metamethod_body(&mut out, mm, &from_dir, opts, layout);
    out
}

fn item_page_from_dir(parent_kind: &str, parent: &str) -> String {
    match parent_kind {
        "module" => format!("modules/{parent}/"),
        "type" | "method" => format!("types/{parent}/"),
        _ => "".to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Inline body fragments (used both standalone and inside parent pages)
// ---------------------------------------------------------------------------

fn render_field_inline(
    out: &mut String,
    f: &FieldDoc,
    from_dir: &str,
    opts: &MdOptions,
    layout: &Layout,
) {
    writeln!(out, "### {} {{#{}}}\n", f.name, field_anchor(&f.name)).ok();
    render_field_body(out, f, from_dir, opts, layout);
}

fn render_field_body(
    out: &mut String,
    f: &FieldDoc,
    from_dir: &str,
    opts: &MdOptions,
    layout: &Layout,
) {
    if let Some(doc) = &f.doc {
        out.push_str(doc);
        out.push_str("\n\n");
    }
    let kind = match f.kind {
        FieldDocKind::Eager => "eager",
        FieldDocKind::Getter => "read-only",
        FieldDocKind::Setter => "write-only",
    };
    writeln!(
        out,
        "- **Type:** {}",
        type_link(&f.ty, from_dir, opts, layout)
    )
    .ok();
    writeln!(out, "- **Access:** {kind}\n").ok();
    render_examples_section(out, f.examples.as_deref());
}

fn render_function_inline(
    out: &mut String,
    func: &FunctionDoc,
    from_dir: &str,
    opts: &MdOptions,
    layout: &Layout,
) {
    writeln!(
        out,
        "### {} {{#{}}}\n",
        func.name,
        function_anchor(&func.name)
    )
    .ok();
    writeln!(out, "```\n{}\n```\n", func.synopsis).ok();
    render_function_body(out, func, from_dir, opts, layout);
}

fn render_metamethod_inline(
    out: &mut String,
    mm: &MetamethodDoc,
    from_dir: &str,
    opts: &MdOptions,
    layout: &Layout,
) {
    writeln!(
        out,
        "### {} {{#{}}}\n",
        mm.method,
        metamethod_anchor(&mm.method)
    )
    .ok();
    writeln!(out, "```\n{}\n```\n", mm.synopsis).ok();
    render_metamethod_body(out, mm, from_dir, opts, layout);
}

fn render_function_body(
    out: &mut String,
    func: &FunctionDoc,
    from_dir: &str,
    opts: &MdOptions,
    layout: &Layout,
) {
    if let Some(doc) = &func.doc {
        out.push_str(doc);
        out.push_str("\n\n");
    }
    render_params_section(
        out,
        &func.params,
        func.variadic.as_ref(),
        func.variadic_doc.as_deref(),
        from_dir,
        opts,
        layout,
    );
    render_returns_section(out, &func.returns, from_dir, opts, layout);
    render_examples_section(out, func.examples.as_deref());
}

fn render_metamethod_body(
    out: &mut String,
    mm: &MetamethodDoc,
    from_dir: &str,
    opts: &MdOptions,
    layout: &Layout,
) {
    if let Some(doc) = &mm.doc {
        out.push_str(doc);
        out.push_str("\n\n");
    }
    render_params_section(
        out,
        &mm.params,
        mm.variadic.as_ref(),
        mm.variadic_doc.as_deref(),
        from_dir,
        opts,
        layout,
    );
    render_returns_section(out, &mm.returns, from_dir, opts, layout);
    render_examples_section(out, mm.examples.as_deref());
}

/// Emit the rustdoc `# Examples` text verbatim under a `**Examples**`
/// header.  Authors include their own fenced code blocks; the
/// emitter only adds the section heading and a trailing blank line.
fn render_examples_section(out: &mut String, examples: Option<&str>) {
    let Some(text) = examples else { return };
    if text.is_empty() {
        return;
    }
    out.push_str("**Examples**\n\n");
    out.push_str(text);
    if !text.ends_with('\n') {
        out.push('\n');
    }
    out.push('\n');
}

fn render_params_section(
    out: &mut String,
    params: &[ParamDoc],
    variadic: Option<&TypeRef>,
    variadic_doc: Option<&str>,
    from_dir: &str,
    opts: &MdOptions,
    layout: &Layout,
) {
    if params.is_empty() && variadic.is_none() {
        return;
    }
    out.push_str("**Parameters**\n\n");
    for p in params {
        let name = p.name.as_deref().unwrap_or("_");
        let opt_marker = if p.optional { " *(optional)*" } else { "" };
        let ty = type_link(&p.ty, from_dir, opts, layout);
        let doc = p.doc.as_deref().unwrap_or("");
        if doc.is_empty() {
            writeln!(out, "- `{name}`: {ty}{opt_marker}").ok();
        } else {
            writeln!(out, "- `{name}`: {ty}{opt_marker} — {doc}").ok();
        }
    }
    if let Some(v) = variadic {
        let ty = type_link(v, from_dir, opts, layout);
        match variadic_doc {
            Some(doc) if !doc.is_empty() => {
                writeln!(out, "- `...`: {ty} — {doc}").ok();
            }
            _ => {
                writeln!(out, "- `...`: {ty}").ok();
            }
        }
    }
    out.push('\n');
}

fn render_returns_section(
    out: &mut String,
    returns: &[ReturnDoc],
    from_dir: &str,
    opts: &MdOptions,
    layout: &Layout,
) {
    if returns.is_empty() {
        return;
    }
    out.push_str("**Returns**\n\n");
    for r in returns {
        let ty = type_link(&r.ty, from_dir, opts, layout);
        let doc = r.doc.as_deref().unwrap_or("");
        if doc.is_empty() {
            writeln!(out, "- {ty}").ok();
        } else {
            writeln!(out, "- {ty} — {doc}").ok();
        }
    }
    out.push('\n');
}

// ---------------------------------------------------------------------------
// Cross-link rendering
// ---------------------------------------------------------------------------

/// Render a [`TypeRef`] as a markdown fragment with linkified
/// references to userdata or module pages.
///
/// Primitive types (no references) render as inline code:
/// `` `string` ``.  Types containing one or more named references
/// render WITHOUT outer backticks, since wrapping a markdown link in
/// backticks would suppress link rendering and produce literal text.
/// Each named reference is replaced by `[name](path)` in place.
fn type_link(ty: &TypeRef, from_dir: &str, opts: &MdOptions, layout: &Layout) -> String {
    let references = ty.references();
    let rendered = display(ty);
    if references.is_empty() {
        return format!("`{rendered}`");
    }
    let mut out = String::new();
    let mut remaining = rendered.as_str();
    while !remaining.is_empty() {
        let mut matched = false;
        for r in &references {
            if remaining.starts_with(r.as_str())
                && is_word_boundary_after(remaining, r.len())
                && is_word_boundary_before(&out)
            {
                let link = match resolve_reference(r, layout) {
                    Some(link) => link,
                    None => continue,
                };
                let target = with_prefix(&join_link(from_dir, &link), opts);
                write!(out, "[{r}]({target})").ok();
                remaining = &remaining[r.len()..];
                matched = true;
                break;
            }
        }
        if !matched {
            let mut chars = remaining.chars();
            if let Some(c) = chars.next() {
                out.push(c);
                remaining = chars.as_str();
            } else {
                break;
            }
        }
    }
    out
}

fn is_word_boundary_before(buf: &str) -> bool {
    match buf.chars().last() {
        None => true,
        Some(c) => !c.is_alphanumeric() && c != '_',
    }
}

fn is_word_boundary_after(s: &str, at: usize) -> bool {
    match s[at..].chars().next() {
        None => true,
        Some(c) => !c.is_alphanumeric() && c != '_',
    }
}

fn resolve_reference(name: &str, layout: &Layout) -> Option<String> {
    if layout.userdata.contains_key(name) {
        Some(format!("types/{name}/index.md"))
    } else if layout.modules.contains_key(name) {
        Some(format!("modules/{name}/index.md"))
    } else {
        None
    }
}

fn module_link(from_dir: &str, name: &str, opts: &MdOptions) -> String {
    with_prefix(
        &join_link(from_dir, &format!("modules/{name}/index.md")),
        opts,
    )
}

fn userdata_link(from_dir: &str, name: &str, opts: &MdOptions) -> String {
    with_prefix(
        &join_link(from_dir, &format!("types/{name}/index.md")),
        opts,
    )
}

/// Join `from_dir` (always ending in `/` or empty) with a target path
/// expressed relative to the output root, producing a path relative
/// to the page that contains the link.
fn join_link(from_dir: &str, target_from_root: &str) -> String {
    let depth = from_dir.matches('/').count();
    if depth == 0 {
        return target_from_root.to_owned();
    }
    let mut prefix = String::new();
    for _ in 0..depth {
        prefix.push_str("../");
    }
    prefix.push_str(target_from_root);
    prefix
}

fn with_prefix(path: &str, opts: &MdOptions) -> String {
    match &opts.link_prefix {
        Some(p) => format!("{p}{path}"),
        None => path.to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Anchors
// ---------------------------------------------------------------------------

fn field_anchor(name: &str) -> String {
    format!("field-{}", slugify(name))
}

fn function_anchor(name: &str) -> String {
    format!("function-{}", slugify(name))
}

fn metamethod_anchor(name: &str) -> String {
    format!("metamethod-{}", slugify(name.trim_start_matches('_')))
}

fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if c == '_' || c == '-' {
            out.push('-');
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Front matter
// ---------------------------------------------------------------------------

fn push_front_matter(out: &mut String, style: FrontMatterStyle, title: &str) {
    match style {
        FrontMatterStyle::None => {}
        FrontMatterStyle::Zensical | FrontMatterStyle::MkDocs => {
            writeln!(out, "---\ntitle: {title}\n---\n").ok();
        }
        FrontMatterStyle::Hugo => {
            writeln!(out, "+++\ntitle = \"{title}\"\n+++\n").ok();
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers on FunctionDoc
// ---------------------------------------------------------------------------

trait FunctionDocExt {
    fn synopsis_anchor_title(&self, parent: &str) -> String;
}

impl FunctionDocExt for FunctionDoc {
    fn synopsis_anchor_title(&self, parent: &str) -> String {
        if self.is_method {
            format!("{parent}:{}", self.name)
        } else {
            format!("{parent}.{}", self.name)
        }
    }
}
