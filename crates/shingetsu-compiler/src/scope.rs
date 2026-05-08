use std::collections::HashMap;

use shingetsu_vm::types::{LocalAttr, LuaType};
use shingetsu_vm::Bytes;

use crate::error::SourceLocation;

/// A local variable tracked during compilation.
#[derive(Debug, Clone)]
pub struct Local {
    pub name: Bytes,
    pub slot: u8,
    pub attr: LocalAttr,
    /// PC at which this local was introduced (for `<close>` crossing checks).
    #[allow(dead_code)]
    pub start_pc: usize,
    /// Number of times this local was read (used for unused-variable warnings).
    pub read_count: u32,
    /// Number of times this local was assigned to after its initial declaration.
    pub write_count: u32,
    /// Source location of the declaration (for diagnostic warnings).
    pub decl_location: Option<SourceLocation>,
    /// Source location of the last assignment after declaration.
    pub last_write_location: Option<SourceLocation>,
    /// Whether this local was declared as a `local function`.
    pub is_function: bool,
    /// Tracks fields defined on this local via `function t.f()` / `function t:m()`.
    /// Maps field name → `true` if defined with `:` (method), `false` if `.` (function).
    pub field_defs: HashMap<Bytes, bool>,
    /// Inferred or annotated type of this local, when known.
    /// Used for compile-time dot-vs-colon checking and cross-module
    /// type propagation.
    pub inferred_type: Option<LuaType>,
    /// Whether this is the implicit `self` parameter of a method declaration.
    pub is_implicit_self: bool,
}

/// Scope manager for a single function being compiled.
pub struct ScopeStack {
    /// Stack of scopes; each scope is a list of locals declared in that scope.
    scopes: Vec<Vec<Local>>,
    /// Next available register slot.
    next_slot: u8,
    /// High-water mark of registers used (for Proto::max_regs).
    pub max_slot: u8,
}

impl ScopeStack {
    pub fn new() -> Self {
        ScopeStack {
            scopes: vec![vec![]],
            next_slot: 0,
            max_slot: 0,
        }
    }

    /// Open a new block scope.
    pub fn push_scope(&mut self) {
        self.scopes.push(vec![]);
    }

    /// Close the innermost block scope, returning the locals that were in it.
    pub fn pop_scope(&mut self) -> Vec<Local> {
        let locals = self.scopes.pop().expect("scope underflow");
        // Reclaim register slots from the closed scope.
        if let Some(first) = locals.first() {
            self.next_slot = first.slot;
        }
        locals
    }

    /// Declare a local variable, allocating a register slot.
    /// Returns an error message string if the slot would overflow u8.
    pub fn declare(
        &mut self,
        name: impl Into<Bytes>,
        attr: LocalAttr,
        pc: usize,
    ) -> Result<u8, String> {
        let name = name.into();
        let slot = self.next_slot;
        if slot == u8::MAX {
            return Err(format!("too many local variables (limit {})", u8::MAX));
        }
        self.next_slot += 1;
        if self.next_slot > self.max_slot {
            self.max_slot = self.next_slot;
        }
        self.scopes
            .last_mut()
            .expect("scope stack is never empty")
            .push(Local {
                name,
                slot,
                attr,
                start_pc: pc,
                read_count: 0,
                write_count: 0,
                decl_location: None,
                last_write_location: None,
                is_function: false,
                field_defs: HashMap::new(),
                inferred_type: None,
                is_implicit_self: false,
            });
        Ok(slot)
    }

    /// Check if a local with the given name exists in the current (innermost) scope.
    /// Returns its declaration location if found.
    pub fn same_scope_lookup(&self, name: &[u8]) -> Option<Option<SourceLocation>> {
        self.scopes.last().and_then(|scope| {
            scope
                .iter()
                .rev()
                .find(|l| l.name.as_ref() == name)
                .map(|l| l.decl_location.clone())
        })
    }

    /// Set the source location on the most recently declared local.
    pub fn set_last_decl_location(&mut self, loc: SourceLocation) {
        if let Some(scope) = self.scopes.last_mut() {
            if let Some(local) = scope.last_mut() {
                local.decl_location = Some(loc);
            }
        }
    }

    /// Set the inferred/annotated type on the most recently declared local.
    pub fn set_last_decl_type(&mut self, lua_type: LuaType) {
        if let Some(scope) = self.scopes.last_mut() {
            if let Some(local) = scope.last_mut() {
                local.inferred_type = Some(lua_type);
            }
        }
    }

    /// Mark the most recently declared local as a function declaration.
    pub fn set_last_decl_is_function(&mut self) {
        if let Some(scope) = self.scopes.last_mut() {
            if let Some(local) = scope.last_mut() {
                local.is_function = true;
            }
        }
    }

    /// Mark the most recently declared local as an implicit `self` parameter.
    pub fn set_last_decl_implicit_self(&mut self) {
        if let Some(scope) = self.scopes.last_mut() {
            if let Some(local) = scope.last_mut() {
                local.is_implicit_self = true;
            }
        }
    }

    /// Look up a local variable by name, searching from innermost scope out.
    /// Returns the most-recently-declared local with that name.
    #[allow(dead_code)]
    pub fn resolve(&self, name: &[u8]) -> Option<&Local> {
        for scope in self.scopes.iter().rev() {
            for local in scope.iter().rev() {
                if local.name.as_ref() == name {
                    return Some(local);
                }
            }
        }
        None
    }

    /// Mutable lookup — same as `resolve` but returns `&mut Local`.
    pub fn resolve_mut(&mut self, name: &[u8]) -> Option<&mut Local> {
        for scope in self.scopes.iter_mut().rev() {
            for local in scope.iter_mut().rev() {
                if local.name.as_ref() == name {
                    return Some(local);
                }
            }
        }
        None
    }

    /// All currently-live locals in innermost-first order.
    #[allow(dead_code)]
    pub fn all_live(&self) -> impl Iterator<Item = &Local> {
        self.scopes.iter().rev().flat_map(|s| s.iter().rev())
    }

    /// Locals in the current (innermost) scope that have `<close>` attribute.
    #[allow(dead_code)]
    pub fn close_vars_in_current_scope(&self) -> impl Iterator<Item = &Local> {
        self.scopes
            .last()
            .map(|s| s.as_slice())
            .unwrap_or(&[])
            .iter()
            .filter(|l| l.attr == LocalAttr::Close)
    }

    /// All live `<close>` locals from every scope that will be exited by a
    /// jump, in reverse declaration order (outermost first, reverse within
    /// scope — so they close in LIFO order).
    ///
    /// `target_depth` is the number of scopes remaining *after* the jump.
    /// Pass 0 to close everything (for `return`).
    #[allow(dead_code)]
    pub fn close_vars_for_exit(&self, target_depth: usize) -> Vec<Local> {
        let scopes_to_exit = self.scopes.len().saturating_sub(target_depth);
        let mut result = Vec::new();
        for scope in self.scopes[target_depth..].iter().rev() {
            let mut close_in_scope: Vec<&Local> = scope
                .iter()
                .filter(|l| l.attr == LocalAttr::Close)
                .collect();
            close_in_scope.reverse();
            result.extend(close_in_scope.into_iter().cloned());
        }
        let _ = scopes_to_exit;
        result
    }

    /// Check whether a jump from the current position to `target_pc` (which
    /// is *inside* a scope at depth `target_depth`) would cross a `<close>`
    /// variable initialisation.  Returns the name of the first such variable
    /// if a crossing is detected.
    ///
    /// A crossing occurs when the target is deeper than the current scope and
    /// a `<close>` variable in an intermediate scope was initialised before
    /// `target_pc`.
    #[allow(dead_code)]
    pub fn check_goto_crossing(&self, _target_depth: usize, _target_pc: usize) -> Option<Bytes> {
        // Stub: `goto` is not supported (LuaU `::` conflict), so this
        // is unreachable.  Would need a full crossing check if `goto`
        // were ever enabled.
        None
    }

    pub fn scope_depth(&self) -> usize {
        self.scopes.len()
    }

    pub fn current_slot(&self) -> u8 {
        self.next_slot
    }
}
