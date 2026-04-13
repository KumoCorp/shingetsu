use std::sync::atomic::{AtomicU8, Ordering};

/// Tri-colour mark for the mark-and-sweep GC.
///
/// Objects start White (potentially unreachable).  During the mark phase,
/// reachable objects are turned Gray (found but children not yet scanned),
/// then Black (fully scanned).  After marking, White objects are garbage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum GcColor {
    /// Not yet reached — candidate for collection.
    White = 0,
    /// Reachable but children not yet scanned.
    Gray = 1,
    /// Reachable and all transitive references scanned.
    Black = 2,
}

/// Header embedded in every GC-managed heap object (`TableState`,
/// `LuaFunctionState`).  Uses an `AtomicU8` so reads and writes are safe
/// across threads without a mutex.
pub(crate) struct GcHeader {
    color: AtomicU8,
}

impl GcHeader {
    pub(crate) fn new() -> Self {
        GcHeader {
            color: AtomicU8::new(GcColor::White as u8),
        }
    }

    pub(crate) fn color(&self) -> GcColor {
        match self.color.load(Ordering::Relaxed) {
            1 => GcColor::Gray,
            2 => GcColor::Black,
            _ => GcColor::White,
        }
    }

    pub(crate) fn set_color(&self, c: GcColor) {
        self.color.store(c as u8, Ordering::Relaxed);
    }
}
