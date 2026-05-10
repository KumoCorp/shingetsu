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
use crate::synopsis::{render_synopsis_pretty, DEFAULT_SYNOPSIS_WIDTH};
use crate::{
    DocExample, DocModel, EventDoc, FieldDoc, FieldDocKind, FunctionDoc, MetamethodDoc, ModuleDoc,
    ParamDoc, ReturnDoc, TypeRef, UserdataDoc,
};

/// Wrap an event's stored single-line synopsis to multiple lines when
/// it exceeds [`DEFAULT_SYNOPSIS_WIDTH`].  Events store only `Vec<TypeRef>`
/// for returns, so we adapt them to `ReturnDoc` for the shared renderer.
fn pretty_event_synopsis(ev: &EventDoc) -> String {
    let returns: Vec<ReturnDoc> = ev
        .returns
        .iter()
        .cloned()
        .map(|ty| ReturnDoc { ty, doc: None })
        .collect();
    render_synopsis_pretty(
        "",
        &ev.name,
        &ev.params,
        None,
        &returns,
        false,
        DEFAULT_SYNOPSIS_WIDTH,
    )
}

fn pretty_function_synopsis(parent: &str, func: &FunctionDoc) -> String {
    render_synopsis_pretty(
        parent,
        &func.name,
        &func.params,
        func.variadic.as_ref(),
        &func.returns,
        func.is_method,
        DEFAULT_SYNOPSIS_WIDTH,
    )
}

fn pretty_metamethod_synopsis(parent: &str, mm: &MetamethodDoc) -> String {
    render_synopsis_pretty(
        parent,
        &mm.method,
        &mm.params,
        mm.variadic.as_ref(),
        &mm.returns,
        false,
        DEFAULT_SYNOPSIS_WIDTH,
    )
}

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

/// Render a navigation fragment as a single TOML inline-table value.
///
/// The fragment represents the reference subtree as a `"Reference"`
/// section containing `"Modules"` and (if any) `"Types"` subsections,
/// each module/type expanded into one entry per addressable item.
/// It is intended to be textually substituted into a zensical or
/// mkdocs `nav` array (which is also a list of TOML inline-tables)
/// in place of a sentinel string.
///
/// `prefix` is the path, relative to the consuming site's docs
/// directory, at which the rendered reference subtree is mounted.
/// Pass an empty string when the reference sits at the docs root.
/// Forward slashes are used regardless of host platform.
pub fn render_nav_fragment(model: &DocModel, opts: &MdOptions, prefix: &str) -> String {
    let layout = Layout::compute(model, opts);
    let p = if prefix.is_empty() {
        String::new()
    } else {
        format!("{}/", prefix.trim_end_matches('/'))
    };

    let mut out = String::new();
    out.push_str("{ \"Reference\" = [\n");
    writeln!(out, "  \"{p}index.md\",").ok();

    if !model.modules.is_empty() {
        out.push_str("  { \"Modules\" = [\n");
        for m in sorted_by_name(&model.modules, |m| &m.name) {
            let display = crate::display_parent(&m.name);
            let module_index = format!("{p}modules/{}/index.md", m.name);
            if m.fields.is_empty() && m.functions.is_empty() {
                writeln!(
                    out,
                    "    {{ {} = \"{module_index}\" }},",
                    toml_quote(&m.name),
                )
                .ok();
                continue;
            }
            writeln!(out, "    {{ {} = [", toml_quote(&m.name)).ok();
            writeln!(out, "      \"{module_index}\",").ok();
            for fld in sorted_by_name(&m.fields, |f| &f.name) {
                writeln!(
                    out,
                    "      {{ {} = \"{p}modules/{}/{}.md\" }},",
                    toml_quote(&qualified(display, &fld.name)),
                    m.name,
                    fld.name,
                )
                .ok();
            }
            for func in sorted_by_name(&m.functions, |f| &f.name) {
                writeln!(
                    out,
                    "      {{ {} = \"{p}modules/{}/{}.md\" }},",
                    toml_quote(&qualified(display, &func.name)),
                    m.name,
                    func.name,
                )
                .ok();
            }
            out.push_str("    ] },\n");
        }
        out.push_str("  ] },\n");
    }

    if !model.events.is_empty() {
        out.push_str("  { \"Events\" = [\n");
        let events_index = format!("{p}events/index.md");
        writeln!(out, "    \"{events_index}\",").ok();
        for ev in sorted_by_name(&model.events, |e| &e.name) {
            writeln!(
                out,
                "    {{ {} = \"{p}events/{}.md\" }},",
                toml_quote(&ev.name),
                ev.name,
            )
            .ok();
        }
        out.push_str("  ] },\n");
    }

    if !model.userdata_types.is_empty() {
        out.push_str("  { \"Types\" = [\n");
        for ud in sorted_by_name(&model.userdata_types, |u| &u.name) {
            let type_index = format!("{p}types/{}/index.md", ud.name);
            let split = layout.userdata_mode(&ud.name) == SplitMode::Split;
            let any_items =
                !ud.fields.is_empty() || !ud.methods.is_empty() || !ud.metamethods.is_empty();
            if !split || !any_items {
                writeln!(
                    out,
                    "    {{ {} = \"{type_index}\" }},",
                    toml_quote(&ud.name),
                )
                .ok();
                continue;
            }
            writeln!(out, "    {{ {} = [", toml_quote(&ud.name)).ok();
            writeln!(out, "      \"{type_index}\",").ok();
            for fld in sorted_by_name(&ud.fields, |f| &f.name) {
                writeln!(
                    out,
                    "      {{ {} = \"{p}types/{}/{}.md\" }},",
                    toml_quote(&qualified(&ud.name, &fld.name)),
                    ud.name,
                    fld.name,
                )
                .ok();
            }
            for meth in sorted_by_name(&ud.methods, |f| &f.name) {
                writeln!(
                    out,
                    "      {{ {} = \"{p}types/{}/{}.md\" }},",
                    toml_quote(&qualified(&ud.name, &meth.name)),
                    ud.name,
                    meth.name,
                )
                .ok();
            }
            for mm in sorted_by_name(&ud.metamethods, |m| &m.method) {
                writeln!(
                    out,
                    "      {{ {} = \"{p}types/{}/{}.md\" }},",
                    toml_quote(&qualified(&ud.name, &mm.method)),
                    ud.name,
                    mm.method,
                )
                .ok();
            }
            out.push_str("    ] },\n");
        }
        out.push_str("  ] },\n");
    }

    out.push_str("] }\n");
    out
}

/// Quote a string as a TOML basic string.
fn toml_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => write!(out, "\\u{:04X}", c as u32).unwrap(),
            c => out.push(c),
        }
    }
    out.push('"');
    out
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

    for m in sorted_by_name(&model.modules, |m| &m.name) {
        let mode = layout.module_mode(&m.name);
        let parent_path = format!("modules/{}/index.md", m.name);
        files.push(MdFile {
            path: PathBuf::from(&parent_path),
            content: render_module_parent(m, mode, opts, &layout),
        });
        if mode == SplitMode::Split {
            for fld in sorted_by_name(&m.fields, |f| &f.name) {
                files.push(MdFile {
                    path: PathBuf::from(format!("modules/{}/{}.md", m.name, fld.name)),
                    content: render_field_page(&m.name, "module", fld, opts, &layout),
                });
            }
            for func in sorted_by_name(&m.functions, |f| &f.name) {
                files.push(MdFile {
                    path: PathBuf::from(format!("modules/{}/{}.md", m.name, func.name)),
                    content: render_function_page(&m.name, "module", func, opts, &layout),
                });
            }
        }
    }

    if !model.events.is_empty() {
        files.push(MdFile {
            path: PathBuf::from("events/index.md"),
            content: render_events_index(model, opts),
        });
        for ev in sorted_by_name(&model.events, |e| &e.name) {
            files.push(MdFile {
                path: PathBuf::from(format!("events/{}.md", ev.name)),
                content: render_event_page(ev, opts, &layout),
            });
        }
    }

    for ud in sorted_by_name(&model.userdata_types, |u| &u.name) {
        let mode = layout.userdata_mode(&ud.name);
        files.push(MdFile {
            path: PathBuf::from(format!("types/{}/index.md", ud.name)),
            content: render_userdata_parent(ud, mode, opts, &layout),
        });
        if mode == SplitMode::Split {
            for fld in sorted_by_name(&ud.fields, |f| &f.name) {
                files.push(MdFile {
                    path: PathBuf::from(format!("types/{}/{}.md", ud.name, fld.name)),
                    content: render_field_page(&ud.name, "type", fld, opts, &layout),
                });
            }
            for m in sorted_by_name(&ud.methods, |f| &f.name) {
                files.push(MdFile {
                    path: PathBuf::from(format!("types/{}/{}.md", ud.name, m.name)),
                    content: render_function_page(&ud.name, "type", m, opts, &layout),
                });
            }
            for mm in sorted_by_name(&ud.metamethods, |m| &m.method) {
                files.push(MdFile {
                    path: PathBuf::from(format!("types/{}/{}.md", ud.name, mm.method)),
                    content: render_metamethod_page(&ud.name, mm, opts, &layout),
                });
            }
        }
    }

    files
}

/// Return references to `items` sorted by `key`, so emitted lists
/// don't expose source-declaration order.
fn sorted_by_name<'a, T, F: Fn(&T) -> &str>(items: &'a [T], key: F) -> Vec<&'a T> {
    let mut v: Vec<&T> = items.iter().collect();
    v.sort_by(|a, b| key(a).cmp(key(b)));
    v
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
            // Modules always split: every function/field gets its own
            // page so it has a stable URL for cross-page linking.
            modules.insert(m.name.clone(), SplitMode::Split);
        }
        let mut userdata = HashMap::new();
        for ud in &model.userdata_types {
            let count = ud.fields.len() + ud.methods.len() + ud.metamethods.len();
            userdata.insert(ud.name.clone(), pick_mode(&ud.name, count, opts));
        }
        Layout { modules, userdata }
    }

    fn module_mode(&self, _name: &str) -> SplitMode {
        SplitMode::Split
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
        for m in sorted_by_name(&model.modules, |m| &m.name) {
            write_index_entry(
                &mut out,
                &format!("`{}`", m.name),
                &module_link("", &m.name, opts),
                m.doc.as_deref(),
            );
        }
        out.push('\n');
    }

    if !model.events.is_empty() {
        out.push_str("## Events\n\n");
        write_index_entry(&mut out, "All events", &events_index_link("", opts), None);
        out.push('\n');
    }

    if !model.userdata_types.is_empty() {
        out.push_str("## Types\n\n");
        for ud in sorted_by_name(&model.userdata_types, |u| &u.name) {
            write_index_entry(
                &mut out,
                &format!("`{}`", ud.name),
                &userdata_link("", &ud.name, opts),
                ud.doc.as_deref(),
            );
        }
        out.push('\n');
    }

    out
}

fn render_events_index(model: &DocModel, opts: &MdOptions) -> String {
    let mut out = String::new();
    push_front_matter(&mut out, opts.front_matter, "Events");
    out.push_str("# Events\n\n");
    for ev in sorted_by_name(&model.events, |e| &e.name) {
        write_index_entry(
            &mut out,
            &format!("`{}`", ev.synopsis),
            &format!("{}.md", ev.name),
            ev.doc.as_deref(),
        );
    }
    out.push('\n');
    out
}

fn render_event_page(ev: &EventDoc, opts: &MdOptions, layout: &Layout) -> String {
    let from_dir = "events/".to_owned();
    let mut out = String::new();
    push_front_matter(&mut out, opts.front_matter, &ev.name);
    writeln!(out, "# {}\n", ev.name).ok();
    writeln!(out, "```\n{}\n```\n", pretty_event_synopsis(ev)).ok();
    if let Some(doc) = &ev.doc {
        out.push_str(doc);
        out.push_str("\n\n");
    }
    render_params_section(&mut out, &ev.params, None, None, &from_dir, opts, layout);
    if !ev.returns.is_empty() {
        out.push_str("**Returns**\n\n");
        let types = ev
            .returns
            .iter()
            .map(|t| type_link(t, &from_dir, opts, layout))
            .collect::<Vec<_>>()
            .join(", ");
        match ev.return_doc.as_deref() {
            Some(doc) if !doc.is_empty() => {
                writeln!(out, "- {types} -- {doc}").ok();
            }
            _ => {
                writeln!(out, "- {types}").ok();
            }
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
                for f in sorted_by_name(&m.fields, |f| &f.name) {
                    render_field_inline(&mut out, f, &from_dir, opts, layout);
                }
            }
            if !m.functions.is_empty() {
                out.push_str("## Functions\n\n");
                for func in sorted_by_name(&m.functions, |f| &f.name) {
                    render_function_inline(&mut out, &m.name, func, &from_dir, opts, layout);
                }
            }
        }
        SplitMode::Split => {
            if !m.fields.is_empty() {
                out.push_str("## Fields\n\n");
                for f in sorted_by_name(&m.fields, |f| &f.name) {
                    write_index_entry(
                        &mut out,
                        &format!("`{}`", f.name),
                        &format!("{}.md", f.name),
                        f.doc.as_deref(),
                    );
                }
                out.push('\n');
            }
            if !m.functions.is_empty() {
                out.push_str("## Functions\n\n");
                for func in sorted_by_name(&m.functions, |f| &f.name) {
                    write_index_entry(
                        &mut out,
                        &format!("`{}`", func.synopsis),
                        &format!("{}.md", func.name),
                        func.doc.as_deref(),
                    );
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
                for f in sorted_by_name(&ud.fields, |f| &f.name) {
                    render_field_inline(&mut out, f, &from_dir, opts, layout);
                }
            }
            if !ud.methods.is_empty() {
                out.push_str("## Methods\n\n");
                for m in sorted_by_name(&ud.methods, |f| &f.name) {
                    render_function_inline(&mut out, &ud.name, m, &from_dir, opts, layout);
                }
            }
            if !ud.metamethods.is_empty() {
                out.push_str("## Metamethods\n\n");
                for mm in sorted_by_name(&ud.metamethods, |m| &m.method) {
                    render_metamethod_inline(&mut out, &ud.name, mm, &from_dir, opts, layout);
                }
            }
        }
        SplitMode::Split => {
            if !ud.fields.is_empty() {
                out.push_str("## Fields\n\n");
                for f in sorted_by_name(&ud.fields, |f| &f.name) {
                    write_index_entry(
                        &mut out,
                        &format!("`{}`", f.name),
                        &format!("{}.md", f.name),
                        f.doc.as_deref(),
                    );
                }
                out.push('\n');
            }
            if !ud.methods.is_empty() {
                out.push_str("## Methods\n\n");
                for m in sorted_by_name(&ud.methods, |f| &f.name) {
                    write_index_entry(
                        &mut out,
                        &format!("`{}`", m.synopsis),
                        &format!("{}.md", m.name),
                        m.doc.as_deref(),
                    );
                }
                out.push('\n');
            }
            if !ud.metamethods.is_empty() {
                out.push_str("## Metamethods\n\n");
                for mm in sorted_by_name(&ud.metamethods, |m| &m.method) {
                    write_index_entry(
                        &mut out,
                        &format!("`{}`", mm.synopsis),
                        &format!("{}.md", mm.method),
                        mm.doc.as_deref(),
                    );
                }
                out.push('\n');
            }
        }
    }

    out
}

/// Emit a single bullet `- [text](href) — brief` for a parent
/// page's section listing, where `brief` is the first paragraph of
/// the item's doc (or omitted entirely when the item has none).
///
/// Showing only the brief avoids duplicating the full prose from
/// the per-item page and keeps the index scannable.
fn write_index_entry(out: &mut String, text: &str, href: &str, doc: Option<&str>) {
    let brief = doc.map(brief_summary).unwrap_or_default();
    if brief.is_empty() {
        writeln!(out, "- [{text}]({href})").ok();
    } else {
        writeln!(out, "- [{text}]({href}) — {brief}").ok();
    }
}

/// Take the first paragraph of `doc` and join its lines into a
/// single space-separated string.  Returns the empty string when
/// the doc is empty or whitespace-only.
fn brief_summary(doc: &str) -> String {
    let mut out = String::new();
    for line in doc.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !out.is_empty() {
                break;
            }
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(trimmed);
    }
    out
}

/// Format `parent.child` for headings and front-matter, dropping the
/// dot when `parent` is empty (the `builtins` module case — its
/// functions are bound as bare globals).
fn qualified(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_owned()
    } else {
        format!("{parent}.{child}")
    }
}

fn render_field_page(
    parent: &str,
    parent_kind: &str,
    f: &FieldDoc,
    opts: &MdOptions,
    layout: &Layout,
) -> String {
    let from_dir = item_page_from_dir(parent_kind, parent);
    let display = crate::display_parent(parent);
    let mut out = String::new();
    push_front_matter(&mut out, opts.front_matter, &qualified(display, &f.name));
    writeln!(out, "# {}\n", qualified(display, &f.name)).ok();
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
    let display = crate::display_parent(parent);
    let title = func.synopsis_anchor_title(display);
    let mut out = String::new();
    // Front-matter title is the short qualified name (e.g.
    // `task.join`); the full synopsis goes in the fenced code
    // block below the heading.  Type expressions in the synopsis
    // contain `[integer]`-style brackets that downstream YAML
    // consumers (e.g. zensical) parse as markdown reference link
    // labels, raising spurious "unresolved link reference"
    // warnings if used as the title.
    push_front_matter(&mut out, opts.front_matter, &title);
    writeln!(out, "# {title}\n").ok();
    writeln!(
        out,
        "```\n{}\n```\n",
        pretty_function_synopsis(display, func)
    )
    .ok();
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
    let display = crate::display_parent(parent);
    let mut out = String::new();
    push_front_matter(&mut out, opts.front_matter, &qualified(display, &mm.method));
    writeln!(out, "# {}\n", qualified(display, &mm.method)).ok();
    writeln!(
        out,
        "```\n{}\n```\n",
        pretty_metamethod_synopsis(display, mm)
    )
    .ok();
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
    writeln!(
        out,
        "- **Type:** {}",
        type_link(&f.ty, from_dir, opts, layout)
    )
    .ok();
    // Only call out access when it constrains what users can do;
    // read-write fields behave like ordinary table entries and
    // don't need annotation.
    let access = match f.kind {
        FieldDocKind::ReadWrite => None,
        FieldDocKind::Getter => Some("read-only"),
        FieldDocKind::Setter => Some("write-only"),
    };
    if let Some(label) = access {
        writeln!(out, "- **Access:** {label}").ok();
    }
    out.push('\n');
    render_examples_section(out, &f.examples);
}

fn render_function_inline(
    out: &mut String,
    parent: &str,
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
    writeln!(
        out,
        "```\n{}\n```\n",
        pretty_function_synopsis(parent, func)
    )
    .ok();
    render_function_body(out, func, from_dir, opts, layout);
}

fn render_metamethod_inline(
    out: &mut String,
    parent: &str,
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
    writeln!(
        out,
        "```\n{}\n```\n",
        pretty_metamethod_synopsis(parent, mm)
    )
    .ok();
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
    render_examples_section(out, &func.examples);
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
    render_examples_section(out, &mm.examples);
}

/// Render the structured `# Examples` section as a `**Examples**`
/// header followed by each example block.  Each block emits its
/// preceding prose (if any), a fenced code block in its declared
/// language, and — when populated by
/// [`crate::populate_example_outputs`] — a trailing
/// `output:` text block showing the captured stdout.
fn render_examples_section(out: &mut String, examples: &[DocExample]) {
    if examples.is_empty() {
        return;
    }
    out.push_str("**Examples**\n\n");
    for ex in examples {
        if let Some(prose) = &ex.prose {
            out.push_str(prose);
            out.push_str("\n\n");
        }
        writeln!(out, "```{}", ex.language).ok();
        out.push_str(&ex.code);
        if !ex.code.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("```\n");
        if let Some(output) = &ex.output {
            if !output.is_empty() {
                out.push_str("\noutput:\n\n```text\n");
                out.push_str(output);
                if !output.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str("```\n");
            }
        }
        out.push('\n');
    }
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
                // Escape `[` / `]` outside of inserted links so
                // CommonMark doesn't try to resolve them as
                // reference-style link labels (e.g. `{[integer]:
                // Task}` from a `Vec<Ud<Task>>` type render).
                if matches!(c, '[' | ']') {
                    out.push('\\');
                }
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

fn events_index_link(from_dir: &str, opts: &MdOptions) -> String {
    with_prefix(&join_link(from_dir, "events/index.md"), opts)
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
            let escaped = yaml_single_quote(title);
            writeln!(out, "---\ntitle: {escaped}\n---\n").ok();
        }
        FrontMatterStyle::Hugo => {
            let escaped = toml_double_quote(title);
            writeln!(out, "+++\ntitle = {escaped}\n+++\n").ok();
        }
    }
}

/// Wrap a string in single quotes for YAML, doubling any embedded single
/// quotes (the YAML escape for a literal single quote inside a single-quoted
/// scalar).  Single-quoted YAML strings are literal otherwise, which avoids
/// having to worry about characters like `:`, `{`, `}`, `#`, etc.
fn yaml_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Wrap a string in double quotes for TOML, escaping `\\` and `"`.
fn toml_double_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------------
// Helpers on FunctionDoc
// ---------------------------------------------------------------------------

trait FunctionDocExt {
    fn synopsis_anchor_title(&self, parent: &str) -> String;
}

impl FunctionDocExt for FunctionDoc {
    fn synopsis_anchor_title(&self, parent: &str) -> String {
        if parent.is_empty() {
            self.name.clone()
        } else if self.is_method {
            format!("{parent}:{}", self.name)
        } else {
            format!("{parent}.{}", self.name)
        }
    }
}
