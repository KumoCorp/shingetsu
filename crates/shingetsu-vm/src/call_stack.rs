use std::sync::Arc;

use crate::byte_string::Bytes;
use crate::proto::SourceLocation;
use crate::types::FunctionSignature;
use crate::value::Value;

/// A single entry in the persistent call stack.
#[derive(Clone, Debug)]
pub enum StackFrame {
    /// A Lua function frame, frozen at the point of a nested call.
    Lua {
        /// Signature of the Lua function (name, params, return types, etc.).
        function: Arc<FunctionSignature>,
        /// Source location at the time this frame was frozen (the `Call`
        /// instruction site).  `None` for the live top frame when no
        /// source-location data is available.
        source_location: Option<SourceLocation>,
        /// Live local variables at the time of the snapshot, in declaration
        /// order.  Always empty in the persistent `CallStack`; only
        /// populated by the error-path snapshot for `detect_hints` and
        /// `debug.getlocal`.
        locals: Vec<(Bytes, Value)>,
        /// Whether the most recent `Call` instruction from this frame used
        /// `:` syntax.  Used by `detect_hints` to suggest `.` vs `:` corrections.
        last_call_is_method: bool,
        /// Byte offset and length of the `.` or `:` token at the most recent
        /// call site.  Used by `detect_hints` to point the hint at the exact token.
        last_call_dot_colon: Option<(u32, u32)>,
        /// Byte offset of the start of the receiver expression at the most
        /// recent call site.
        last_call_receiver_offset: Option<u32>,
        /// Signature of the callee for the most recent `Call` instruction.
        /// Used by `detect_hints` when the callee frame is not on the stack.
        last_call_callee_sig: Option<Arc<FunctionSignature>>,
    },
    /// A native (host) function frame.
    Native {
        /// Name of the native function.
        function_name: Bytes,
    },
}

impl StackFrame {
    /// Create a Lua stack frame with only a function signature.
    ///
    /// All other fields default to `None`/`false`/empty.  Source location
    /// is set later via `CallStack::set_top_source_location` when the
    /// frame is frozen by a nested call.
    pub fn lua(function: Arc<FunctionSignature>) -> Self {
        Self::Lua {
            function,
            source_location: None,
            locals: vec![],
            last_call_is_method: false,
            last_call_dot_colon: None,
            last_call_receiver_offset: None,
            last_call_callee_sig: None,
        }
    }
}

/// A copy-on-write call stack backed by `Arc<Vec<StackFrame>>`.
///
/// Cloning (snapshotting) is O(1) — just an Arc refcount bump.
/// Push, pop, and mutation use `Arc::make_mut` for copy-on-write:
/// when the refcount is 1 (the common case on the hot path), this
/// is a no-op; when a snapshot is outstanding, it triggers a single
/// Vec clone before mutating.
#[derive(Clone, Debug)]
pub struct CallStack {
    frames: Arc<Vec<StackFrame>>,
}

impl Default for CallStack {
    fn default() -> Self {
        Self::new()
    }
}

impl CallStack {
    /// An empty call stack.
    pub fn new() -> Self {
        Self {
            frames: Arc::new(Vec::new()),
        }
    }

    /// Push a new frame onto the stack.  O(1) when no snapshot is
    /// outstanding; O(n) COW clone otherwise (amortised rare).
    #[inline]
    pub fn push(&mut self, entry: StackFrame) {
        Arc::make_mut(&mut self.frames).push(entry);
    }

    /// Pop the top frame.  O(1) when no snapshot is outstanding.
    #[inline]
    pub fn pop(&mut self) -> Option<StackFrame> {
        Arc::make_mut(&mut self.frames).pop()
    }

    /// Number of frames on the stack.
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Whether the stack is empty.
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Iterate frames from outermost (bottom) to innermost (top).
    pub fn frames_bottom_up(&self) -> &[StackFrame] {
        &self.frames
    }

    /// Iterate frames from innermost (top) to outermost (bottom).
    pub fn frames_top_down(&self) -> impl Iterator<Item = &StackFrame> {
        self.frames.iter().rev()
    }

    /// Collect all frames into a `Vec`, outermost first.
    pub fn to_vec(&self) -> Vec<StackFrame> {
        self.frames.as_ref().clone()
    }

    /// Peek at the top (innermost) frame without removing it.
    pub fn top(&self) -> Option<&StackFrame> {
        self.frames.last()
    }

    /// Update the source location of the top Lua frame.
    ///
    /// Uses `Arc::make_mut` for copy-on-write — only clones the Vec
    /// if a snapshot is outstanding.
    ///
    /// If the top frame is not a `Lua` frame, this is a no-op.
    #[inline]
    pub fn set_top_source_location(&mut self, loc: Option<SourceLocation>) {
        if let Some(StackFrame::Lua {
            ref mut source_location,
            ..
        }) = Arc::make_mut(&mut self.frames).last_mut()
        {
            *source_location = loc;
        }
    }
}

/// Read-only view of the call stack with local-variable values populated.
///
/// Built on demand for native functions that opt in via a `FrameLocals`
/// parameter (e.g. `debug.getlocal`).  Frames are stored LIFO — the
/// most recently called frame will be the last frame in the vector.
pub struct FrameLocals {
    frames: Vec<StackFrame>,
}

impl FrameLocals {
    pub fn new(frames: Vec<StackFrame>) -> Self {
        Self { frames }
    }

    /// Get the frame at `level` (0 = most recent / top of stack).
    pub fn frame(&self, level: usize) -> Option<&StackFrame> {
        let idx = self.frames.len().checked_sub(1 + level)?;
        self.frames.get(idx)
    }

    /// Get the local variable at 1-based `index` in the frame at `level`.
    pub fn get_local(&self, level: usize, index: usize) -> Option<(Bytes, Value)> {
        match self.frame(level)? {
            StackFrame::Lua { locals, .. } => {
                let i = index.checked_sub(1)?;
                locals.get(i).cloned()
            }
            StackFrame::Native { .. } => None,
        }
    }

    /// Number of visible locals at `level`.
    pub fn local_count(&self, level: usize) -> usize {
        match self.frame(level) {
            Some(StackFrame::Lua { locals, .. }) => locals.len(),
            _ => 0,
        }
    }

    /// Total number of frames.
    pub fn depth(&self) -> usize {
        self.frames.len()
    }

    /// The underlying frames, outermost first.
    pub fn frames(&self) -> &[StackFrame] {
        &self.frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lua_frame(name: &str) -> StackFrame {
        StackFrame::lua(Arc::new(FunctionSignature {
            name: Bytes::from(name),
            source: Bytes::default(),
            type_params: vec![],
            params: vec![],
            variadic: false,
            arg_offset: 0,
            returns: None,
            lua_returns: None,
            line_defined: 0,
            last_line_defined: 0,
            num_upvalues: 0,
            has_runtime_types: false,
        }))
    }

    fn native_frame(name: &str) -> StackFrame {
        StackFrame::Native {
            function_name: Bytes::from(name),
        }
    }

    #[test]
    fn push_pop_basic() {
        let mut stack = CallStack::new();
        k9::assert_equal!(stack.len(), 0);
        assert!(stack.is_empty());

        stack.push(lua_frame("main"));
        stack.push(lua_frame("foo"));
        stack.push(native_frame("print"));
        k9::assert_equal!(stack.len(), 3);

        let names: Vec<&str> = stack
            .frames_bottom_up()
            .iter()
            .map(|f| match f {
                StackFrame::Lua { function, .. } => std::str::from_utf8(&function.name).unwrap(),
                StackFrame::Native { function_name } => std::str::from_utf8(function_name).unwrap(),
            })
            .collect();
        k9::assert_equal!(names, vec!["main", "foo", "print"]);

        stack.pop();
        k9::assert_equal!(stack.len(), 2);

        stack.pop();
        stack.pop();
        assert!(stack.is_empty());
        assert!(stack.pop().is_none());
    }

    #[test]
    fn snapshot_shares_structure() {
        let mut stack = CallStack::new();
        stack.push(lua_frame("a"));
        stack.push(lua_frame("b"));

        let snapshot = stack.clone();

        stack.push(lua_frame("c"));
        k9::assert_equal!(stack.len(), 3);
        k9::assert_equal!(snapshot.len(), 2);

        let snap_names: Vec<&str> = snapshot
            .frames_bottom_up()
            .iter()
            .map(|f| match f {
                StackFrame::Lua { function, .. } => std::str::from_utf8(&function.name).unwrap(),
                _ => unreachable!(),
            })
            .collect();
        k9::assert_equal!(snap_names, vec!["a", "b"]);
    }

    #[test]
    fn top_down_iteration() {
        let mut stack = CallStack::new();
        stack.push(lua_frame("bottom"));
        stack.push(lua_frame("middle"));
        stack.push(lua_frame("top"));

        let names: Vec<&str> = stack
            .frames_top_down()
            .map(|f| match f {
                StackFrame::Lua { function, .. } => std::str::from_utf8(&function.name).unwrap(),
                _ => unreachable!(),
            })
            .collect();
        k9::assert_equal!(names, vec!["top", "middle", "bottom"]);
    }

    #[test]
    fn cow_preserves_snapshot() {
        let mut stack = CallStack::new();
        stack.push(lua_frame("a"));
        stack.push(lua_frame("b"));

        // Snapshot shares the Arc.
        let snapshot = stack.clone();

        // Mutation triggers COW — snapshot is unaffected.
        stack.push(lua_frame("c"));
        k9::assert_equal!(stack.len(), 3);
        k9::assert_equal!(snapshot.len(), 2);

        // Pop also triggers COW.
        stack.pop();
        stack.pop();
        k9::assert_equal!(stack.len(), 1);
        k9::assert_equal!(snapshot.len(), 2);
    }

    #[test]
    fn set_top_source_location_cow() {
        let mut stack = CallStack::new();
        stack.push(lua_frame("main"));
        let loc = SourceLocation {
            source_name: "test.lua".into(),
            line: 10,
            column: 1,
            byte_offset: 0,
            byte_len: 0,
        };
        stack.set_top_source_location(Some(loc.clone()));

        // Verify location was set.
        match stack.top() {
            Some(StackFrame::Lua {
                source_location, ..
            }) => {
                k9::assert_equal!(source_location.as_ref().map(|l| l.line), Some(10));
            }
            _ => panic!("expected Lua frame"),
        }

        // Take a snapshot, then mutate — snapshot retains old location.
        let snapshot = stack.clone();
        let loc2 = SourceLocation {
            source_name: "test.lua".into(),
            line: 20,
            column: 5,
            byte_offset: 100,
            byte_len: 0,
        };
        stack.set_top_source_location(Some(loc2));

        match stack.top() {
            Some(StackFrame::Lua {
                source_location, ..
            }) => {
                k9::assert_equal!(source_location.as_ref().map(|l| l.line), Some(20));
            }
            _ => panic!("expected Lua frame"),
        }
        match snapshot.top() {
            Some(StackFrame::Lua {
                source_location, ..
            }) => {
                k9::assert_equal!(source_location.as_ref().map(|l| l.line), Some(10));
            }
            _ => panic!("expected Lua frame"),
        }
    }

    #[test]
    fn set_top_source_location_native_is_noop() {
        let mut stack = CallStack::new();
        stack.push(native_frame("print"));
        let loc = SourceLocation {
            source_name: "test.lua".into(),
            line: 1,
            column: 1,
            byte_offset: 0,
            byte_len: 0,
        };
        stack.set_top_source_location(Some(loc));
        // Native frame should be unchanged.
        match stack.top() {
            Some(StackFrame::Native { function_name }) => {
                k9::assert_equal!(function_name.as_ref(), b"print");
            }
            _ => panic!("expected Native frame"),
        }
    }

    #[test]
    fn set_top_source_location_empty_is_noop() {
        let mut stack = CallStack::new();
        let loc = SourceLocation {
            source_name: "test.lua".into(),
            line: 1,
            column: 1,
            byte_offset: 0,
            byte_len: 0,
        };
        // Should not panic on empty stack.
        stack.set_top_source_location(Some(loc));
        assert!(stack.is_empty());
    }

    #[test]
    fn multiple_snapshots_independent() {
        let mut stack = CallStack::new();
        stack.push(lua_frame("a"));
        let snap1 = stack.clone();

        stack.push(lua_frame("b"));
        let snap2 = stack.clone();

        stack.push(lua_frame("c"));

        k9::assert_equal!(snap1.len(), 1);
        k9::assert_equal!(snap2.len(), 2);
        k9::assert_equal!(stack.len(), 3);

        // Popping the live stack doesn't affect either snapshot.
        stack.pop();
        stack.pop();
        stack.pop();
        assert!(stack.is_empty());
        k9::assert_equal!(snap1.len(), 1);
        k9::assert_equal!(snap2.len(), 2);
    }

    #[test]
    fn to_vec_returns_owned_copy() {
        let mut stack = CallStack::new();
        stack.push(lua_frame("x"));
        stack.push(lua_frame("y"));

        let v = stack.to_vec();
        k9::assert_equal!(v.len(), 2);

        // Modifying the stack doesn't affect the returned Vec.
        stack.pop();
        k9::assert_equal!(v.len(), 2);
        k9::assert_equal!(stack.len(), 1);
    }

    #[test]
    fn frames_bottom_up_order() {
        let mut stack = CallStack::new();
        stack.push(lua_frame("first"));
        stack.push(lua_frame("second"));
        stack.push(lua_frame("third"));

        let names: Vec<&str> = stack
            .frames_bottom_up()
            .iter()
            .map(|f| match f {
                StackFrame::Lua { function, .. } => std::str::from_utf8(&function.name).unwrap(),
                _ => unreachable!(),
            })
            .collect();
        k9::assert_equal!(names, vec!["first", "second", "third"]);
    }
}
