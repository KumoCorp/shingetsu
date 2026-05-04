//! `shingetsu doc` subcommand: produce reference documentation from
//! a populated [`shingetsu::GlobalEnv`].
//!
//! Three actions:
//!
//! - `dump-json` — extract the [`DocModel`] and serialize it as JSON.
//!   This is the canonical interchange format; downstream tools
//!   (markdown emitters, doc site builders) consume it via
//!   `render-markdown` or via their own `serde_json` deserialization.
//! - `render-luau` — emit a `.d.luau` definition file for use with
//!   [luau-lsp](https://github.com/JohnnyMorganz/luau-lsp).
//! - `render-markdown` — read a JSON export and produce a self-contained
//!   subtree of markdown pages.  The two-step JSON-then-markdown flow
//!   lets embedders (kumomta, wezterm) generate JSON from their own
//!   `GlobalEnv` and pipe it through the shingetsu binary without
//!   linking the markdown emitter into their build.
//!
//! Library equivalents live in [`shingetsu_docgen`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use clap::{Args, Subcommand, ValueEnum};
use shingetsu::{GlobalEnv, Libraries};
use shingetsu_docgen::{
    extract, render_luau, render_markdown, DocModel, FrontMatterStyle, MdOptions,
};

#[derive(Subcommand)]
pub enum DocAction {
    /// Extract docs from the standard preloaded `GlobalEnv` and write
    /// them as JSON.  Pass `--out -` (or omit `--out`) to write to
    /// stdout.
    DumpJson(DumpJsonArgs),

    /// Emit a luau-lsp compatible `.d.luau` definition file.
    RenderLuau(RenderLuauArgs),

    /// Render markdown pages from a JSON export.
    RenderMarkdown(RenderMarkdownArgs),
}

#[derive(Args)]
pub struct DumpJsonArgs {
    /// Output file path.  Use `-` or omit to write to stdout.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Library set to register before extracting (default: all).
    #[arg(long, value_parser = crate::parse_libraries)]
    libraries: Option<Libraries>,
}

#[derive(Args)]
pub struct RenderLuauArgs {
    /// Output file path.  Use `-` or omit to write to stdout.
    #[arg(long)]
    out: Option<PathBuf>,
    #[arg(long, value_parser = crate::parse_libraries)]
    libraries: Option<Libraries>,
}

#[derive(Args)]
pub struct RenderMarkdownArgs {
    /// JSON export produced by `shingetsu doc dump-json`.
    #[arg(long)]
    input: PathBuf,
    /// Output directory.  Created if missing; existing files are
    /// overwritten.
    #[arg(long)]
    out: PathBuf,
    /// Front-matter style for emitted pages.
    #[arg(long, value_enum, default_value_t = FrontMatterArg::None)]
    front_matter: FrontMatterArg,
    /// Emit a separate page per item once a module/type has more than
    /// this many items.  Default: 12.  Use `0` to always split,
    /// or a very large number to always inline.
    #[arg(long, default_value_t = 12)]
    split_threshold: usize,
    /// Optional URL prefix prepended to all generated cross-page
    /// links.  Useful when the generated subtree is mounted under a
    /// non-root path in the consuming site.
    #[arg(long)]
    link_prefix: Option<String>,
}

#[derive(Clone, Copy, ValueEnum)]
enum FrontMatterArg {
    None,
    Zensical,
    Mkdocs,
    Hugo,
}

impl From<FrontMatterArg> for FrontMatterStyle {
    fn from(a: FrontMatterArg) -> Self {
        match a {
            FrontMatterArg::None => FrontMatterStyle::None,
            FrontMatterArg::Zensical => FrontMatterStyle::Zensical,
            FrontMatterArg::Mkdocs => FrontMatterStyle::MkDocs,
            FrontMatterArg::Hugo => FrontMatterStyle::Hugo,
        }
    }
}

pub fn run(action: DocAction) -> Result<()> {
    match action {
        DocAction::DumpJson(args) => dump_json(args),
        DocAction::RenderLuau(args) => render_luau_cmd(args),
        DocAction::RenderMarkdown(args) => render_markdown_cmd(args),
    }
}

fn build_env(libraries: Option<Libraries>) -> Result<GlobalEnv> {
    let env = GlobalEnv::new();
    shingetsu::register_libs(&env, libraries.unwrap_or(Libraries::ALL))?;
    Ok(env)
}

fn dump_json(args: DumpJsonArgs) -> Result<()> {
    let env = build_env(args.libraries)?;
    let model = extract(&env);
    let json = serde_json::to_string_pretty(&model).context("serializing DocModel")?;
    write_text(args.out.as_deref(), &json)
}

fn render_luau_cmd(args: RenderLuauArgs) -> Result<()> {
    let env = build_env(args.libraries)?;
    let text = render_luau(&extract(&env));
    write_text(args.out.as_deref(), &text)
}

fn render_markdown_cmd(args: RenderMarkdownArgs) -> Result<()> {
    let json = std::fs::read_to_string(&args.input)
        .with_context(|| format!("reading {}", args.input.display()))?;
    let model: DocModel =
        serde_json::from_str(&json).with_context(|| format!("parsing {}", args.input.display()))?;

    let opts = MdOptions {
        front_matter: args.front_matter.into(),
        split_threshold: args.split_threshold,
        split_overrides: HashMap::new(),
        link_prefix: args.link_prefix,
    };
    let files = render_markdown(&model, &opts);

    std::fs::create_dir_all(&args.out)
        .with_context(|| format!("creating {}", args.out.display()))?;
    for f in &files {
        let path = args.out.join(&f.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&path, &f.content).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

/// Write `text` to `path`.  `None` or `Some("-")` writes to stdout.
fn write_text(path: Option<&Path>, text: &str) -> Result<()> {
    match path {
        None => {
            print!("{text}");
            Ok(())
        }
        Some(p) if p == Path::new("-") => {
            print!("{text}");
            Ok(())
        }
        Some(p) => {
            if let Some(parent) = p.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)
                        .with_context(|| format!("creating {}", parent.display()))?;
                }
            }
            std::fs::write(p, text).with_context(|| format!("writing {}", p.display()))
        }
    }
}
