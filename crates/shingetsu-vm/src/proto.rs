use std::collections::BTreeMap;
use std::sync::Arc;

use crate::byte_string::Bytes;

use crate::types::{FunctionSignature, LocalAttr};
use crate::value::Value;

/// Source location embedded in bytecode for stack traces.
#[derive(Debug, Clone)]
pub struct SourceLocation {
    pub source_name: String,
    pub line: u32,
    pub column: u32,
    /// Byte offset from the start of the source text.
    pub byte_offset: u32,
    /// Length in bytes of the span (0 = point / unknown).
    pub byte_len: u32,
}

impl std::fmt::Display for SourceLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}:{}:{}",
            format_source_name(&self.source_name),
            self.line,
            self.column
        )
    }
}

const SOURCE_NAME_MAX_LEN: usize = 60;

/// Format a `source_name` for display in error messages and tracebacks,
/// following Lua 5.4's `luaO_chunkid` conventions:
///
/// - `@path` → file path, shown as-is (without the `@`).  Truncated from
///   the front with `...` if longer than 60 characters.
/// - `=label` → embedder label, shown as-is (without the `=`).  Truncated
///   from the end with `...` if longer than 60 characters.
/// - Anything else → source text.  Shown as `[string "first line..."]`,
///   truncated at 60 characters or the first newline.
pub fn format_source_name(name: &str) -> String {
    if let Some(path) = name.strip_prefix('@') {
        if path.len() <= SOURCE_NAME_MAX_LEN {
            path.to_owned()
        } else {
            let tail = truncate_left(path, SOURCE_NAME_MAX_LEN - 3);
            format!("...{tail}")
        }
    } else if let Some(label) = name.strip_prefix('=') {
        if label.len() <= SOURCE_NAME_MAX_LEN {
            label.to_owned()
        } else {
            let head = truncate_right(label, SOURCE_NAME_MAX_LEN - 3);
            format!("{head}...")
        }
    } else {
        let first_line = name.lines().next().unwrap_or(name);
        let overhead = "[string \"...\"]".len();
        let max_content = SOURCE_NAME_MAX_LEN.saturating_sub(overhead);
        if first_line.len() <= max_content {
            format!("[string \"{first_line}\"]")
        } else {
            let head = truncate_right(first_line, max_content);
            format!("[string \"{head}...\"]")
        }
    }
}

/// Return the longest suffix of `s` whose byte length is at most `max_bytes`,
/// retreating to the previous char boundary if `max_bytes` lands mid-character.
fn truncate_left(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let start = s.len() - max_bytes;
    let start = s.ceil_char_boundary(start);
    &s[start..]
}

/// Return the longest prefix of `s` whose byte length is at most `max_bytes`,
/// retreating to the previous char boundary if `max_bytes` lands mid-character.
fn truncate_right(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let end = s.floor_char_boundary(max_bytes);
    &s[..end]
}

/// Descriptor for a local variable in a `Proto`.
#[derive(Debug, Clone)]
pub struct LocalDesc {
    pub name: Bytes,
    pub attr: LocalAttr,
    /// Register slot.
    pub slot: u8,
    /// PC at which the local comes into scope (inclusive).
    pub start_pc: usize,
    /// PC at which the local goes out of scope (exclusive).
    pub end_pc: usize,
    /// Source location of the declaration (for diagnostics).
    pub decl_location: Option<SourceLocation>,
    /// Whether this is the implicit `self` parameter of a `:` method.
    pub is_implicit_self: bool,
}

/// Descriptor for an upvalue captured by a `Proto`.
#[derive(Debug, Clone)]
pub struct UpvalueDesc {
    pub name: Bytes,
    /// If `true`, captured from the immediately enclosing function's register.
    /// If `false`, captured from that function's upvalue list.
    pub in_stack: bool,
    /// Register or upvalue index in the enclosing function.
    pub index: u8,
}

/// Debug info for a `Call` instruction's call site, recording the
/// position of the `.` or `:` token so that diagnostic hints can
/// point at the exact token, and the receiver expression span so
/// hints can name the actual variable (e.g. `c:add()` not `obj:add()`).
#[derive(Debug, Clone)]
pub struct CallSiteInfo {
    /// Byte offset of the `.` or `:` token from the start of the source.
    pub dot_colon_offset: u32,
    /// Byte length of the `.` or `:` token (always 1, but stored for
    /// consistency with `SourceLocation`).
    pub dot_colon_len: u32,
    /// Byte offset of the start of the receiver expression.
    /// The receiver text is `source[receiver_offset..dot_colon_offset]`.
    pub receiver_offset: u32,
}

/// A compiled function prototype — the static, shareable unit of bytecode.
#[derive(Debug)]
pub struct Proto {
    pub signature: Arc<FunctionSignature>,
    pub code: Vec<u32>,
    /// Constant pool referenced by `LoadK`, `GetGlobal`, etc.
    pub constants: Vec<Value>,
    pub locals: Vec<LocalDesc>,
    pub upvalues: Vec<UpvalueDesc>,
    /// Nested function prototypes (closures defined inside this function).
    pub protos: Vec<Arc<Proto>>,
    /// Per-instruction source locations, parallel to `instructions`.
    /// Empty when `debug_info` is false.
    pub source_locations: Vec<Option<SourceLocation>>,
    /// Sparse per-instruction call-site debug info, keyed by PC.
    /// Only populated for `Call` instructions when `debug_info` is true.
    pub call_site_info: BTreeMap<usize, CallSiteInfo>,
    /// Original source text, shared across all `Proto`s from the same
    /// compilation.  Used by diagnostic rendering to show annotated
    /// source snippets.
    pub source_text: Bytes,
    /// `type Name = ...` aliases declared in this function scope.
    /// Compile-time metadata only — no runtime effect.
    pub type_aliases: std::collections::HashMap<Bytes, crate::types::TypeAlias>,
    /// Maximum register slot used by this function (locals + temporaries).
    /// Used to pre-allocate the register file so `get`/`set` avoid bounds
    /// checks at runtime.
    pub max_stack_size: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- @path convention (file paths) ------------------------------------

    #[test]
    fn at_short_path() {
        k9::assert_equal!(format_source_name("@main.lua"), "main.lua");
    }

    #[test]
    fn at_exact_60_chars() {
        let path = "a".repeat(SOURCE_NAME_MAX_LEN);
        k9::assert_equal!(format_source_name(&format!("@{path}")), path);
    }

    #[test]
    fn at_long_path_truncated_from_front() {
        let path = "a".repeat(SOURCE_NAME_MAX_LEN + 10);
        let result = format_source_name(&format!("@{path}"));
        k9::assert_equal!(result.len(), SOURCE_NAME_MAX_LEN);
        assert!(result.starts_with("..."));
        k9::assert_equal!(
            result,
            format!("...{}", &path[path.len() - (SOURCE_NAME_MAX_LEN - 3)..])
        );
    }

    // -- =label convention ------------------------------------------------

    #[test]
    fn eq_short_label() {
        k9::assert_equal!(format_source_name("=<string>"), "<string>");
    }

    #[test]
    fn eq_exact_60_chars() {
        let label = "b".repeat(SOURCE_NAME_MAX_LEN);
        k9::assert_equal!(format_source_name(&format!("={label}")), label);
    }

    #[test]
    fn eq_long_label_truncated_from_end() {
        let label = "b".repeat(SOURCE_NAME_MAX_LEN + 5);
        let result = format_source_name(&format!("={label}"));
        k9::assert_equal!(result.len(), SOURCE_NAME_MAX_LEN);
        assert!(result.ends_with("..."));
        k9::assert_equal!(result, format!("{}...", &label[..SOURCE_NAME_MAX_LEN - 3]));
    }

    // -- source text (no prefix) ------------------------------------------

    #[test]
    fn source_short_single_line() {
        k9::assert_equal!(format_source_name("return 42"), "[string \"return 42\"]");
    }

    #[test]
    fn source_long_single_line_truncated() {
        let long = "x".repeat(200);
        let result = format_source_name(&long);
        assert!(result.len() <= SOURCE_NAME_MAX_LEN);
        assert!(result.starts_with("[string \""));
        assert!(result.ends_with("...\"]"));
    }

    #[test]
    fn source_multi_line_uses_first_line_only() {
        k9::assert_equal!(
            format_source_name("line1\nline2\nline3"),
            "[string \"line1\"]"
        );
    }

    #[test]
    fn source_empty_string() {
        k9::assert_equal!(format_source_name(""), "[string \"\"]");
    }

    // -- multi-byte UTF-8 at truncation boundary --------------------------

    #[test]
    fn at_long_path_multibyte_does_not_panic() {
        // Build a path that forces truncation right at a multi-byte char.
        // U+00E9 (é) is 2 bytes in UTF-8.
        let filler = "a".repeat(SOURCE_NAME_MAX_LEN);
        let path = format!("{filler}é");
        let result = format_source_name(&format!("@{path}"));
        // Must not panic, and must be valid UTF-8 (it's a String).
        assert!(result.starts_with("..."));
        assert!(result.len() <= SOURCE_NAME_MAX_LEN);
    }

    #[test]
    fn eq_long_label_multibyte_does_not_panic() {
        // Place 2-byte chars so the naive byte slice would land inside one.
        let filler = "a".repeat(SOURCE_NAME_MAX_LEN - 2);
        let label = format!("{filler}ééé");
        assert!(label.len() > SOURCE_NAME_MAX_LEN);
        let result = format_source_name(&format!("={label}"));
        assert!(result.ends_with("..."));
        assert!(result.len() <= SOURCE_NAME_MAX_LEN);
    }

    #[test]
    fn source_long_multibyte_does_not_panic() {
        let overhead = "[string \"...\"]".len();
        let max_content = SOURCE_NAME_MAX_LEN - overhead;
        // Place a 2-byte char right at the truncation boundary.
        let filler = "a".repeat(max_content);
        let src = format!("{filler}é more stuff");
        let result = format_source_name(&src);
        assert!(result.starts_with("[string \""));
        assert!(result.ends_with("...\"]"));
        assert!(result.len() <= SOURCE_NAME_MAX_LEN);
    }

    // -- edge cases -------------------------------------------------------

    #[test]
    fn at_bare_prefix_gives_empty_path() {
        k9::assert_equal!(format_source_name("@"), "");
    }

    #[test]
    fn eq_bare_prefix_gives_empty_label() {
        k9::assert_equal!(format_source_name("="), "");
    }

    #[test]
    fn at_prefix_only_whitespace() {
        k9::assert_equal!(format_source_name("@ "), " ");
    }

    #[test]
    fn eq_prefix_only_whitespace() {
        k9::assert_equal!(format_source_name("= "), " ");
    }
}

impl Proto {
    /// Set source text on this proto and all nested child protos.
    /// Uses `Bytes` cheap cloning so all protos share one allocation.
    /// Set source text on this proto and all nested child protos.
    /// Uses `Bytes` cheap cloning so all protos share one allocation.
    ///
    /// Must be called before any `Arc<Proto>` is shared (i.e. while
    /// each child proto has a unique reference).
    pub fn set_source_text(&mut self, source: Bytes) {
        self.source_text = source.clone();
        for child in &mut self.protos {
            Arc::get_mut(child)
                .expect("Proto already shared before set_source_text")
                .set_source_text(source.clone());
        }
    }
}
