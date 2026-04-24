//! Lua 5.4-style traceback rendering with typed signature annotations.
//!
//! This module provides two pure functions:
//!
//! * [`render_frame`] — renders a single [`StackFrame`] into a one-line
//!   traceback entry.
//! * [`render_traceback`] — assembles a full traceback from a stack
//!   snapshot, with an optional leading message and `level`-based
//!   frame skipping.
//!
//! ## Design decisions
//!
//! * **Lua 5.4 format** — each frame is `<location>: in <descriptor>`.
//! * **Typed signatures** — parameter and return types from the source
//!   annotations are included: `function inner(foo: number): number`.
//!   Type source priority: [`LuaType`] → [`ValueType`] → omitted.
//! * **`[Native]` label** — native frames use `[Native]` instead of
//!   Lua 5.4's `[C]` since no C language is involved.
//! * **No bookend noise** — synthetic top-most or bottom-most native
//!   entries that would just show `[Native]: in ?` are suppressed.
//! * **Leaf names only** — native functions show their leaf name (e.g.
//!   `getenv`, not `os.getenv`).  Namespace resolution is deferred.
//! * **`<anonymous>` sentinel** — a function whose name is literally
//!   `<anonymous>` is rendered as `function <source:linedefined>(…)`
//!   instead of `function <anonymous>(…)`.
//!
//! [`LuaType`]: crate::types::LuaType
//! [`ValueType`]: crate::types::ValueType

use std::fmt::Write;

use bstr::BStr;

use crate::call_stack::StackFrame;
use crate::types::{FunctionSignature, ParamSpec, ValueType};

/// The sentinel name the compiler assigns to anonymous function
/// expressions (see `lower.rs`).
const ANONYMOUS_SENTINEL: &[u8] = b"<anonymous>";

/// Render a single stack frame as a one-line traceback entry.
///
/// `is_main_chunk` should be `true` for the outermost Lua frame (the
/// top-level file/chunk); it changes the descriptor from
/// `in function …` to `in main chunk`.
///
/// # Examples
///
/// ```text
/// test.lua:3: in function inner(foo: number): number
/// [Native]: in function getenv(varname: string): string?
/// test.lua:1: in main chunk
/// ```
pub fn render_frame(frame: &StackFrame, is_main_chunk: bool) -> String {
    match frame {
        StackFrame::Lua {
            function,
            source_location,
            ..
        } => {
            let mut out = String::new();
            // Location prefix: "source:line" or just "?" if unavailable.
            if let Some(loc) = source_location {
                write!(
                    out,
                    "{}:{}",
                    crate::proto::format_source_name(&loc.source_name),
                    loc.line
                )
                .ok();
            } else {
                out.push('?');
            }
            out.push_str(": in ");
            if is_main_chunk {
                out.push_str("main chunk");
            } else {
                render_lua_descriptor(&mut out, function);
            }
            out
        }
        StackFrame::Native { function_name } => {
            let name = BStr::new(function_name);
            if function_name.is_empty() {
                // Unnamed native — render as `[Native]: in ?`
                "[Native]: in ?".to_owned()
            } else {
                // Named native — for now we only have the leaf name,
                // not the full signature.  Once StackFrame::Native
                // carries Arc<FunctionSignature>, this branch should
                // render typed params like the Lua frame does.
                format!("[Native]: in function {name}")
            }
        }
    }
}

/// Assemble a full Lua 5.4-style traceback from a stack snapshot.
///
/// * `stack` — outermost frame first (same order as
///   [`CallContext::call_stack`]).
/// * `message` — optional string prepended before the `stack traceback:`
///   header.  If `message` is `Some` but the value is empty, the header
///   still appears on its own line.  Non-string messages in Lua semantics
///   are passed through by the caller; this function always prepends.
/// * `level` — number of frames to skip from the *top* (innermost end)
///   of the stack.  `0` includes the caller's own frame; `1` (the
///   default for `debug.traceback`) skips it.
///
/// The traceback is returned as a single `String` with embedded newlines.
///
/// [`CallContext::call_stack`]: crate::call_context::CallContext
pub fn render_traceback(stack: &[StackFrame], message: Option<&str>, level: usize) -> String {
    let mut out = String::new();
    if let Some(msg) = message {
        out.push_str(msg);
        out.push('\n');
    }
    out.push_str("stack traceback:");

    // The stack is outermost-first; the traceback prints innermost-first
    // (most recent call at the top).  Reverse, then skip `level` frames.
    let frames: Vec<_> = stack.iter().rev().collect();
    let frames = if level < frames.len() {
        &frames[level..]
    } else {
        &[]
    };

    for (i, frame) in frames.iter().enumerate() {
        // Bookend suppression: skip a Native frame that sits at the very
        // bottom of the displayed stack (i.e. last in iteration) when it
        // would just print "[Native]: in ?" with no useful information.
        if i == frames.len() - 1 {
            if let StackFrame::Native { function_name } = frame {
                if function_name.is_empty() {
                    break;
                }
            }
        }

        // The outermost Lua frame (last in the reversed+displayed list)
        // is the main chunk.
        let is_main_chunk = is_main_chunk_frame(frame, frames);

        out.push_str("\n\t");
        out.push_str(&render_frame(frame, is_main_chunk));
    }

    out
}

/// Determine whether `frame` is the main-chunk frame within the given
/// displayed frame list.
///
/// The main chunk is the outermost (last in the reversed display order)
/// Lua frame.  We scan backwards to find it, skipping trailing Native
/// frames.
fn is_main_chunk_frame(frame: &StackFrame, frames: &[&StackFrame]) -> bool {
    if !matches!(frame, StackFrame::Lua { .. }) {
        return false;
    }
    // Walk from the end to find the outermost Lua frame.
    for f in frames.iter().rev() {
        if matches!(f, StackFrame::Lua { .. }) {
            return std::ptr::eq(*f, frame);
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Descriptor rendering helpers
// ---------------------------------------------------------------------------

/// Render the descriptor portion of a Lua frame: `function name(params): returns`
/// or `function <source:linedefined>(params): returns` for anonymous functions.
fn render_lua_descriptor(out: &mut String, sig: &FunctionSignature) {
    out.push_str("function ");

    let name = &sig.name;
    if name == ANONYMOUS_SENTINEL || name.is_empty() {
        // Anonymous — we don't have source/linedefined on the signature
        // itself (it lives on Proto), so render `<anonymous>`.  The
        // traceback assembler could enrich this in the future.
        out.push_str("<anonymous>");
    } else {
        let name = BStr::new(name);
        write!(out, "{name}").ok();
    }

    render_signature(out, sig);
}

/// Render the `(params): returns` portion of a function signature.
///
/// * Parameters are rendered as `name: type` when both are available,
///   `name` when only a name is present, or just the type when only a
///   type is present.  The type source priority is: `lua_type` (from
///   source annotations) → `runtime_type` (derived from Rust bindings)
///   → omitted.
/// * Variadic functions append `...` (untyped) or `...: type` (typed)
///   after the last named parameter.
/// * The return clause is omitted entirely when no return types are
///   known.  A single return renders as `: T`; multiple returns render
///   as `: (A, B)`.
pub fn render_signature(out: &mut String, sig: &FunctionSignature) {
    out.push('(');

    // Skip `arg_offset` leading params (e.g. the implicit `self` in methods).
    let visible_params = if sig.arg_offset <= sig.params.len() {
        &sig.params[sig.arg_offset..]
    } else {
        &[]
    };

    let mut first = true;
    for p in visible_params {
        if !first {
            out.push_str(", ");
        }
        first = false;
        render_param(out, p);
    }

    if sig.variadic {
        if !first {
            out.push_str(", ");
        }
        out.push_str("...");
    }
    out.push(')');

    // Return clause — omit entirely when no return types are known.
    render_return_clause(out, sig);
}

/// Render a single parameter as `name: type`, `name`, or `type`.
fn render_param(out: &mut String, p: &ParamSpec) {
    let type_str = param_type_string(p);
    match (&p.name, type_str.as_deref()) {
        (Some(name), Some(ty)) => {
            let name = BStr::new(name);
            write!(out, "{name}: {ty}").ok();
        }
        (Some(name), None) => {
            let name = BStr::new(name);
            write!(out, "{name}").ok();
        }
        (None, Some(ty)) => {
            out.push_str(ty);
        }
        (None, None) => {
            // No name and no type — render nothing visible (shouldn't
            // normally happen, but defensive).
        }
    }
}

/// Derive the display string for a parameter's type.
///
/// Priority: `lua_type` (source annotation) → `runtime_type` (Rust
/// binding) → `None`.
fn param_type_string(p: &ParamSpec) -> Option<String> {
    if let Some(lt) = &p.lua_type {
        return Some(lt.to_string());
    }
    if let Some(rt) = &p.runtime_type {
        return Some(value_type_display(rt));
    }
    None
}

/// Render the return-type clause: `: T` or `: (A, B)`.
///
/// Omits the clause entirely when no return types are available.
/// Prefers `lua_returns` over `returns` (same priority as params).
fn render_return_clause(out: &mut String, sig: &FunctionSignature) {
    if let Some(lua_rets) = &sig.lua_returns {
        if lua_rets.is_empty() {
            return;
        }
        out.push_str(": ");
        if lua_rets.len() == 1 {
            write!(out, "{}", lua_rets[0]).ok();
        } else {
            out.push('(');
            for (i, r) in lua_rets.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write!(out, "{r}").ok();
            }
            out.push(')');
        }
        return;
    }
    if let Some(rt_rets) = &sig.returns {
        if rt_rets.is_empty() {
            return;
        }
        out.push_str(": ");
        if rt_rets.len() == 1 {
            out.push_str(&value_type_display(&rt_rets[0]));
        } else {
            out.push('(');
            for (i, r) in rt_rets.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&value_type_display(r));
            }
            out.push(')');
        }
    }
    // No return info at all — omit the clause.
}

/// Render a [`ValueType`] as a Luau-style type name.
fn value_type_display(vt: &ValueType) -> String {
    match vt {
        ValueType::Nil => "nil".to_owned(),
        ValueType::Boolean => "boolean".to_owned(),
        ValueType::Integer => "integer".to_owned(),
        ValueType::Float => "float".to_owned(),
        ValueType::Number => "number".to_owned(),
        ValueType::String => "string".to_owned(),
        ValueType::Table => "table".to_owned(),
        ValueType::Function => "function".to_owned(),
        ValueType::Userdata => "userdata".to_owned(),
        ValueType::UserdataOf(name) => (*name).to_owned(),
        ValueType::Any => "any".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::byte_string::Bytes;
    use crate::proto::SourceLocation;
    use crate::types::LuaType;

    fn n(s: &str) -> Bytes {
        Bytes::from(s.as_bytes())
    }

    fn sig(
        name: &str,
        params: Vec<ParamSpec>,
        lua_returns: Option<Vec<LuaType>>,
    ) -> Arc<FunctionSignature> {
        Arc::new(FunctionSignature {
            name: n(name),
            source: Bytes::default(),
            type_params: vec![],
            params,
            variadic: false,
            arg_offset: 0,
            returns: None,
            lua_returns,
            line_defined: 0,
            last_line_defined: 0,
            num_upvalues: 0,
        })
    }

    fn lua_frame(function: Arc<FunctionSignature>, source: &str, line: u32) -> StackFrame {
        StackFrame::Lua {
            function,
            source_location: Some(SourceLocation {
                source_name: source.to_owned(),
                line,
                column: 0,
                byte_offset: 0,
                byte_len: 0,
            }),
            locals: vec![],
            last_call_is_method: false,
            last_call_dot_colon: None,
            last_call_receiver_offset: None,
            last_call_callee_sig: None,
        }
    }

    fn native_frame(name: &str) -> StackFrame {
        StackFrame::Native {
            function_name: n(name),
        }
    }

    // ---- Frame rendering: Lua frames -----------------------------------------

    #[test]
    fn frame_fully_typed_lua() {
        let s = sig(
            "inner",
            vec![ParamSpec {
                name: Some(n("foo")),
                runtime_type: None,
                lua_type: Some(LuaType::Number),
            }],
            Some(vec![LuaType::Number]),
        );
        let frame = lua_frame(s, "@test.lua", 3);
        k9::assert_equal!(
            render_frame(&frame, false),
            "test.lua:3: in function inner(foo: number): number"
        );
    }

    #[test]
    fn frame_untyped_lua() {
        let s = sig(
            "greet",
            vec![ParamSpec {
                name: Some(n("name")),
                runtime_type: None,
                lua_type: None,
            }],
            None,
        );
        let frame = lua_frame(s, "@hello.lua", 10);
        k9::assert_equal!(
            render_frame(&frame, false),
            "hello.lua:10: in function greet(name)"
        );
    }

    #[test]
    fn frame_partially_typed() {
        let s = sig(
            "mix",
            vec![
                ParamSpec {
                    name: Some(n("a")),
                    runtime_type: None,
                    lua_type: Some(LuaType::Number),
                },
                ParamSpec {
                    name: Some(n("b")),
                    runtime_type: None,
                    lua_type: None,
                },
            ],
            None,
        );
        let frame = lua_frame(s, "@test.lua", 5);
        k9::assert_equal!(
            render_frame(&frame, false),
            "test.lua:5: in function mix(a: number, b)"
        );
    }

    #[test]
    fn frame_variadic_typed() {
        let s = Arc::new(FunctionSignature {
            name: n("vfn"),
            source: Bytes::default(),
            type_params: vec![],
            params: vec![ParamSpec {
                name: Some(n("first")),
                runtime_type: None,
                lua_type: Some(LuaType::Number),
            }],
            variadic: true,
            arg_offset: 0,
            returns: None,
            lua_returns: Some(vec![LuaType::String]),
            line_defined: 0,
            last_line_defined: 0,
            num_upvalues: 0,
        });
        let frame = lua_frame(s, "@test.lua", 7);
        k9::assert_equal!(
            render_frame(&frame, false),
            "test.lua:7: in function vfn(first: number, ...): string"
        );
    }

    #[test]
    fn frame_variadic_untyped() {
        let s = Arc::new(FunctionSignature {
            name: n("va"),
            source: Bytes::default(),
            type_params: vec![],
            params: vec![],
            variadic: true,
            arg_offset: 0,
            returns: None,
            lua_returns: None,
            line_defined: 0,
            last_line_defined: 0,
            num_upvalues: 0,
        });
        let frame = lua_frame(s, "@test.lua", 1);
        k9::assert_equal!(
            render_frame(&frame, false),
            "test.lua:1: in function va(...)"
        );
    }

    #[test]
    fn frame_anonymous_function() {
        let s = sig("<anonymous>", vec![], None);
        let frame = lua_frame(s, "@test.lua", 12);
        k9::assert_equal!(
            render_frame(&frame, false),
            "test.lua:12: in function <anonymous>()"
        );
    }

    #[test]
    fn frame_main_chunk() {
        let s = sig("test.lua", vec![], None);
        let frame = lua_frame(s, "@test.lua", 1);
        k9::assert_equal!(render_frame(&frame, true), "test.lua:1: in main chunk");
    }

    #[test]
    fn frame_no_source_location() {
        let s = sig("mystery", vec![], None);
        let frame = StackFrame::lua(s);
        k9::assert_equal!(render_frame(&frame, false), "?: in function mystery()");
    }

    // ---- Frame rendering: Native frames --------------------------------------

    #[test]
    fn frame_named_native() {
        let frame = native_frame("getenv");
        k9::assert_equal!(render_frame(&frame, false), "[Native]: in function getenv");
    }

    #[test]
    fn frame_unnamed_native() {
        let frame = native_frame("");
        k9::assert_equal!(render_frame(&frame, false), "[Native]: in ?");
    }

    // ---- Frame rendering: return-type via runtime_type -----------------------

    #[test]
    fn frame_runtime_type_fallback_for_returns() {
        let s = Arc::new(FunctionSignature {
            name: n("rt_fn"),
            source: Bytes::default(),
            type_params: vec![],
            params: vec![ParamSpec {
                name: Some(n("x")),
                runtime_type: Some(ValueType::Number),
                lua_type: None,
            }],
            variadic: false,
            arg_offset: 0,
            returns: Some(vec![ValueType::String]),
            lua_returns: None,
            line_defined: 0,
            last_line_defined: 0,
            num_upvalues: 0,
        });
        let frame = lua_frame(s, "@test.lua", 1);
        // runtime_type as fallback for both param and return rendering.
        k9::assert_equal!(
            render_frame(&frame, false),
            "test.lua:1: in function rt_fn(x: number): string"
        );
    }

    #[test]
    fn frame_lua_type_takes_priority_over_runtime_type() {
        let s = Arc::new(FunctionSignature {
            name: n("prio"),
            source: Bytes::default(),
            type_params: vec![],
            params: vec![ParamSpec {
                name: Some(n("x")),
                runtime_type: Some(ValueType::Number),
                lua_type: Some(LuaType::Number),
            }],
            variadic: false,
            arg_offset: 0,
            returns: Some(vec![ValueType::String]),
            lua_returns: Some(vec![LuaType::String]),
            line_defined: 0,
            last_line_defined: 0,
            num_upvalues: 0,
        });
        let frame = lua_frame(s, "@test.lua", 1);
        // lua_type wins over runtime_type.
        k9::assert_equal!(
            render_frame(&frame, false),
            "test.lua:1: in function prio(x: number): string"
        );
    }

    #[test]
    fn frame_multiple_returns() {
        let s = sig(
            "multi",
            vec![],
            Some(vec![LuaType::Number, LuaType::String]),
        );
        let frame = lua_frame(s, "@test.lua", 1);
        k9::assert_equal!(
            render_frame(&frame, false),
            "test.lua:1: in function multi(): (number, string)"
        );
    }

    #[test]
    fn frame_method_skips_self() {
        // arg_offset=1 means the first param (self) is hidden.
        let s = Arc::new(FunctionSignature {
            name: n("Foo:bar"),
            source: Bytes::default(),
            type_params: vec![],
            params: vec![
                ParamSpec {
                    name: Some(n("self")),
                    runtime_type: None,
                    lua_type: None,
                },
                ParamSpec {
                    name: Some(n("x")),
                    runtime_type: None,
                    lua_type: Some(LuaType::Number),
                },
            ],
            variadic: false,
            arg_offset: 1,
            returns: None,
            lua_returns: None,
            line_defined: 0,
            last_line_defined: 0,
            num_upvalues: 0,
        });
        let frame = lua_frame(s, "@test.lua", 5);
        k9::assert_equal!(
            render_frame(&frame, false),
            "test.lua:5: in function Foo:bar(x: number)"
        );
    }

    // ---- Full traceback assembly ---------------------------------------------

    #[test]
    fn traceback_basic_stack() {
        let main = lua_frame(sig("test.lua", vec![], None), "@test.lua", 1);
        let foo = lua_frame(
            sig(
                "foo",
                vec![ParamSpec {
                    name: Some(n("x")),
                    runtime_type: None,
                    lua_type: Some(LuaType::Number),
                }],
                Some(vec![LuaType::Number]),
            ),
            "@test.lua",
            5,
        );
        // Stack is outermost-first.
        let stack = vec![main, foo];
        let tb = render_traceback(&stack, None, 0);
        k9::assert_equal!(
            tb,
            "stack traceback:\n\
            \ttest.lua:5: in function foo(x: number): number\n\
            \ttest.lua:1: in main chunk"
        );
    }

    #[test]
    fn traceback_with_message() {
        let main = lua_frame(sig("test.lua", vec![], None), "@test.lua", 1);
        let stack = vec![main];
        let tb = render_traceback(&stack, Some("error: something broke"), 0);
        k9::assert_equal!(
            tb,
            "error: something broke\n\
            stack traceback:\n\
            \ttest.lua:1: in main chunk"
        );
    }

    #[test]
    fn traceback_level_skips_frames() {
        let main = lua_frame(sig("test.lua", vec![], None), "@test.lua", 1);
        let foo = lua_frame(sig("foo", vec![], None), "@test.lua", 3);
        let bar = lua_frame(sig("bar", vec![], None), "@test.lua", 7);
        let stack = vec![main, foo, bar];
        // Level 1 skips the innermost frame (bar).
        let tb = render_traceback(&stack, None, 1);
        k9::assert_equal!(
            tb,
            "stack traceback:\n\
            \ttest.lua:3: in function foo()\n\
            \ttest.lua:1: in main chunk"
        );
    }

    #[test]
    fn traceback_level_past_stack_is_empty() {
        let main = lua_frame(sig("test.lua", vec![], None), "@test.lua", 1);
        let stack = vec![main];
        let tb = render_traceback(&stack, None, 99);
        k9::assert_equal!(tb, "stack traceback:");
    }

    #[test]
    fn traceback_mixed_lua_native() {
        let main = lua_frame(sig("test.lua", vec![], None), "@test.lua", 1);
        let pcall = native_frame("pcall");
        let inner = lua_frame(sig("inner", vec![], None), "@test.lua", 5);
        let stack = vec![main, pcall, inner];
        let tb = render_traceback(&stack, None, 0);
        k9::assert_equal!(
            tb,
            "stack traceback:\n\
            \ttest.lua:5: in function inner()\n\
            \t[Native]: in function pcall\n\
            \ttest.lua:1: in main chunk"
        );
    }

    #[test]
    fn traceback_suppresses_trailing_unnamed_native() {
        // An unnamed native at the bottom of the displayed stack is
        // bookend noise — suppress it.
        let unnamed = native_frame("");
        let main = lua_frame(sig("test.lua", vec![], None), "@test.lua", 1);
        // outermost first: unnamed → main.  Reversed for display: main → unnamed.
        // unnamed is at the bottom and empty → suppressed.
        let stack = vec![unnamed, main];
        let tb = render_traceback(&stack, None, 0);
        k9::assert_equal!(
            tb,
            "stack traceback:\n\
            \ttest.lua:1: in main chunk"
        );
    }

    #[test]
    fn traceback_keeps_trailing_named_native() {
        // A named native at the bottom IS meaningful.
        let setup = native_frame("setup");
        let main = lua_frame(sig("test.lua", vec![], None), "@test.lua", 1);
        let stack = vec![setup, main];
        let tb = render_traceback(&stack, None, 0);
        k9::assert_equal!(
            tb,
            "stack traceback:\n\
            \ttest.lua:1: in main chunk\n\
            \t[Native]: in function setup"
        );
    }

    #[test]
    fn traceback_empty_stack() {
        let tb = render_traceback(&[], None, 0);
        k9::assert_equal!(tb, "stack traceback:");
    }

    #[test]
    fn traceback_anonymous_in_stack() {
        let main = lua_frame(sig("test.lua", vec![], None), "@test.lua", 1);
        let anon = lua_frame(sig("<anonymous>", vec![], None), "@test.lua", 3);
        let stack = vec![main, anon];
        let tb = render_traceback(&stack, None, 0);
        k9::assert_equal!(
            tb,
            "stack traceback:\n\
            \ttest.lua:3: in function <anonymous>()\n\
            \ttest.lua:1: in main chunk"
        );
    }
}
