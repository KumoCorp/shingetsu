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

/// A node in the persistent call-stack linked list.
///
/// Each node points to its parent (the frame below it on the stack).
/// Sharing is via `Arc`, so snapshotting the entire stack is O(1) —
/// just clone the `Arc` to the top node.
#[derive(Debug)]
struct CallStackNode {
    entry: StackFrame,
    parent: Option<Arc<CallStackNode>>,
}

/// A persistent, O(1)-snapshot call stack.
///
/// Internally a singly-linked list of `Arc<CallStackNode>`.  Push and
/// pop are O(1); cloning (snapshotting) is O(1) — it just bumps the
/// top node's refcount.
///
/// Does **not** contain local-variable values.  Those are accessed
/// separately via [`FrameLocals`](crate::frame_locals::FrameLocals)
/// for the rare functions that need them (e.g. `debug.getlocal`).
#[derive(Clone, Debug, Default)]
pub struct CallStack {
    top: Option<Arc<CallStackNode>>,
    len: usize,
}

impl CallStack {
    /// An empty call stack.
    pub fn new() -> Self {
        Self { top: None, len: 0 }
    }

    /// Push a new frame onto the stack.  O(1).
    pub fn push(&mut self, entry: StackFrame) {
        let node = Arc::new(CallStackNode {
            entry,
            parent: self.top.take(),
        });
        self.top = Some(node);
        self.len += 1;
    }

    /// Pop the top frame.  O(1).  Returns `None` if the stack is empty.
    pub fn pop(&mut self) -> Option<StackFrame> {
        let node = self.top.take()?;
        self.len -= 1;
        match Arc::try_unwrap(node) {
            // Sole owner: move the entry out without cloning.
            Ok(owned) => {
                self.top = owned.parent;
                Some(owned.entry)
            }
            // Other clones of this stack share this node (e.g. a snapshot
            // was taken before this pop).  Clone the entry and parent link
            // so the shared snapshot remains valid.
            Err(arc) => {
                self.top = arc.parent.clone();
                Some(arc.entry.clone())
            }
        }
    }

    /// Number of frames on the stack.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the stack is empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Iterate frames from outermost (bottom) to innermost (top).
    ///
    /// This collects into a temporary `Vec` internally because the
    /// linked list is stored top-to-bottom.  Only used on diagnostic /
    /// debug paths, never on the hot dispatch path.
    pub fn frames_bottom_up(&self) -> Vec<&StackFrame> {
        let mut frames = Vec::with_capacity(self.len);
        let mut cur = &self.top;
        while let Some(node) = cur {
            frames.push(&node.entry);
            cur = &node.parent;
        }
        frames.reverse();
        frames
    }

    /// Iterate frames from innermost (top) to outermost (bottom).
    pub fn frames_top_down(&self) -> StackIter<'_> {
        StackIter {
            current: self.top.as_deref(),
        }
    }

    /// Collect all frames into a `Vec`, outermost first.
    pub fn to_vec(&self) -> Vec<StackFrame> {
        self.frames_bottom_up().into_iter().cloned().collect()
    }

    /// Peek at the top (innermost) frame without removing it.
    pub fn top(&self) -> Option<&StackFrame> {
        self.top.as_ref().map(|n| &n.entry)
    }

    /// Update the source location of the top Lua frame.
    ///
    /// Used to freeze the caller's source location when a new frame is
    /// pushed (the caller is suspended at a `Call` instruction and its
    /// PC won't change until the callee returns).
    ///
    /// If the top frame is not a `Lua` frame, this is a no-op.
    pub fn set_top_source_location(&mut self, loc: Option<SourceLocation>) {
        let Some(old) = self.top.take() else { return };
        match Arc::try_unwrap(old) {
            // Sole owner: mutate in place, re-wrap.
            Ok(mut owned) => {
                if let StackFrame::Lua {
                    ref mut source_location,
                    ..
                } = owned.entry
                {
                    *source_location = loc;
                }
                self.top = Some(Arc::new(owned));
            }
            // Shared with a snapshot: clone and modify so the snapshot
            // retains the old source location.
            Err(arc) => {
                let mut entry = arc.entry.clone();
                if let StackFrame::Lua {
                    ref mut source_location,
                    ..
                } = entry
                {
                    *source_location = loc;
                }
                self.top = Some(Arc::new(CallStackNode {
                    entry,
                    parent: arc.parent.clone(),
                }));
            }
        }
    }
}

/// Iterator over stack frames from top (innermost) to bottom (outermost).
pub struct StackIter<'a> {
    current: Option<&'a CallStackNode>,
}

impl<'a> Iterator for StackIter<'a> {
    type Item = &'a StackFrame;

    fn next(&mut self) -> Option<Self::Item> {
        let node = self.current?;
        self.current = node.parent.as_deref();
        Some(&node.entry)
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
}
