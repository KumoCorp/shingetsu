use std::cell::UnsafeCell;
use std::sync::Arc;

use crate::value::Value;

/// The runtime state of an upvalue: either an open pointer into a live
/// frame's pinned register array, or a closed self-contained value.
enum UpvalueState {
    /// Points into the enclosing `LuaFrame`'s pinned register array.
    /// Valid only while the frame is alive; must be closed before the
    /// frame's registers are dropped.
    Open(*mut Value),
    /// Self-contained value, copied from the register when the frame
    /// exited.
    Closed(Value),
}

/// Interior of an upvalue cell.  All access goes through unsafe methods
/// because the `Open` variant holds a raw pointer.
///
/// # Safety
///
/// The raw pointer in `Open` is valid only while the owning `LuaFrame`
/// is alive and its `Pin<Box<[Value]>>` has not been dropped.  Callers
/// must ensure:
/// - `read`/`write` are only called while the frame is alive.
/// - `close` is called before the frame's registers are dropped.
/// - All access is single-threaded within a VM task (no concurrent
///   reads/writes through the pointer).
pub struct UpvalueInner {
    state: UnsafeCell<UpvalueState>,
}

// The VM is single-threaded within a task.  The raw pointer is only
// dereferenced from the owning task's execution loop.  Send+Sync are
// required because `Arc<UpvalueInner>` is stored in `Function` which
// is `Send + Sync`.
unsafe impl Send for UpvalueInner {}
unsafe impl Sync for UpvalueInner {}

impl UpvalueInner {
    /// Create an open upvalue pointing at a register slot.
    ///
    /// # Safety
    ///
    /// `ptr` must point into a pinned allocation that outlives all
    /// `read`/`write` calls made before `close` is called.
    pub unsafe fn new_open(ptr: *mut Value) -> Self {
        Self {
            state: UnsafeCell::new(UpvalueState::Open(ptr)),
        }
    }

    /// Create a closed upvalue owning the given value.
    pub fn new_closed(value: Value) -> Self {
        Self {
            state: UnsafeCell::new(UpvalueState::Closed(value)),
        }
    }

    /// Read the upvalue's current value.
    ///
    /// # Safety
    ///
    /// If the upvalue is open, the pointer must still be valid.
    pub unsafe fn read(&self) -> Value {
        match &*self.state.get() {
            UpvalueState::Open(ptr) => (*(*ptr)).clone(),
            UpvalueState::Closed(v) => v.clone(),
        }
    }

    /// Write a new value into the upvalue.
    ///
    /// # Safety
    ///
    /// If the upvalue is open, the pointer must still be valid.
    pub unsafe fn write(&self, value: Value) {
        match &mut *self.state.get() {
            UpvalueState::Open(ptr) => **ptr = value,
            UpvalueState::Closed(v) => *v = value,
        }
    }

    /// Close the upvalue: if open, copy the pointed-to value into owned
    /// storage and discard the pointer.  No-op if already closed.
    ///
    /// # Safety
    ///
    /// If the upvalue is open, the pointer must still be valid at the
    /// time of this call.
    pub unsafe fn close(&self) {
        let state = &mut *self.state.get();
        if let UpvalueState::Open(ptr) = state {
            *state = UpvalueState::Closed((**ptr).clone());
        }
    }

    /// Returns `true` if this upvalue is still open (pointing into a
    /// live frame's registers).
    pub fn is_open(&self) -> bool {
        // Safe: only reads the discriminant, not the pointer.
        unsafe { matches!(&*self.state.get(), UpvalueState::Open(_)) }
    }

    /// Re-open a closed upvalue with a new pointer.  Used after
    /// register array reallocation to restore the bidirectional sync
    /// between the cell and the register slot.
    ///
    /// # Safety
    ///
    /// `ptr` must point into a pinned allocation that outlives all
    /// subsequent `read`/`write` calls before the next `close`.
    /// The value at `*ptr` must match the cell's current closed value
    /// (caller is responsible for this by copying values into the new
    /// array before calling `reopen`).
    pub unsafe fn reopen(&self, ptr: *mut Value) {
        let state = &mut *self.state.get();
        *state = UpvalueState::Open(ptr);
    }

    /// Returns the raw pointer if the upvalue is open, for sibling
    /// deduplication during `NewClosure`.
    ///
    /// # Safety
    ///
    /// The returned pointer must not be dereferenced unless the caller
    /// knows the frame is still alive.
    pub unsafe fn open_ptr(&self) -> Option<*mut Value> {
        match &*self.state.get() {
            UpvalueState::Open(ptr) => Some(*ptr),
            UpvalueState::Closed(_) => None,
        }
    }
}

impl std::fmt::Debug for UpvalueInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Safe: only reads the discriminant for display purposes.
        unsafe {
            match &*self.state.get() {
                UpvalueState::Open(ptr) => write!(f, "UpvalueInner::Open({ptr:?})"),
                UpvalueState::Closed(v) => write!(f, "UpvalueInner::Closed({v:?})"),
            }
        }
    }
}

/// Shared mutable cell for a captured upvalue.
pub type UpvalueCell = Arc<UpvalueInner>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_upvalue_read_write() {
        let uv = UpvalueInner::new_closed(Value::Integer(42));
        unsafe {
            k9::assert_equal!(uv.read(), Value::Integer(42));
            uv.write(Value::Integer(99));
            k9::assert_equal!(uv.read(), Value::Integer(99));
        }
        assert!(!uv.is_open());
    }

    #[test]
    fn open_upvalue_read_write_close() {
        let mut reg = Value::Integer(10);
        let uv = unsafe { UpvalueInner::new_open(&mut reg as *mut Value) };
        assert!(uv.is_open());
        unsafe {
            k9::assert_equal!(uv.read(), Value::Integer(10));
            uv.write(Value::Integer(20));
            // Write goes through to the register.
            k9::assert_equal!(reg, Value::Integer(20));
            uv.close();
        }
        assert!(!uv.is_open());
        // After close, the upvalue owns its own copy.
        reg = Value::Integer(999);
        unsafe {
            k9::assert_equal!(uv.read(), Value::Integer(20));
        }
    }

    #[test]
    fn close_is_idempotent() {
        let mut reg = Value::Integer(5);
        let uv = unsafe { UpvalueInner::new_open(&mut reg as *mut Value) };
        unsafe {
            uv.close();
            uv.close(); // no-op
            k9::assert_equal!(uv.read(), Value::Integer(5));
        }
    }

    #[test]
    fn reopen_after_close() {
        let mut reg1 = Value::Integer(10);
        let uv = unsafe { UpvalueInner::new_open(&mut reg1 as *mut Value) };
        unsafe {
            uv.close();
        }
        assert!(!uv.is_open());

        // Simulate register reallocation: new array with same value.
        let mut reg2 = Value::Integer(10);
        unsafe {
            uv.reopen(&mut reg2 as *mut Value);
        }
        assert!(uv.is_open());

        // Writes go through to the new register.
        unsafe {
            uv.write(Value::Integer(99));
            k9::assert_equal!(reg2, Value::Integer(99));
            k9::assert_equal!(uv.read(), Value::Integer(99));
        }
    }

    #[test]
    fn sibling_deduplication_via_open_ptr() {
        let mut reg = Value::Integer(1);
        let ptr = &mut reg as *mut Value;
        let uv = unsafe { UpvalueInner::new_open(ptr) };
        unsafe {
            k9::assert_equal!(uv.open_ptr(), Some(ptr));
            uv.close();
            k9::assert_equal!(uv.open_ptr(), None);
        }
    }
}
