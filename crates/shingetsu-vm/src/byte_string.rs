// Ported from the ecow crate (https://github.com/typst/ecow)
// by Typst, licensed under MIT / Apache-2.0.
// Specialized for u8 byte strings with small-string optimization.

use core::alloc::Layout;
use core::borrow::Borrow;
use core::cmp::Ordering;
use core::fmt;
use core::hash::{Hash, Hasher};
use core::mem::{self, ManuallyDrop};
use core::ops::Deref;
use core::ptr::{self, NonNull};
use std::sync::atomic::Ordering::*;
use std::sync::atomic::{self, AtomicUsize};

/// A 16-byte byte string with small-string optimization and O(1) clone.
///
/// Strings of 15 bytes or fewer are stored inline (no heap allocation).
/// Longer strings are stored in a reference-counted heap allocation;
/// cloning bumps an atomic refcount rather than copying data.
#[derive(Clone)]
pub struct Bytes(DynamicVec);

// --- Public API (constructor surface: From impls only) ---

impl Default for Bytes {
    #[inline]
    fn default() -> Self {
        Self(DynamicVec::new())
    }
}

impl From<&str> for Bytes {
    #[inline]
    fn from(s: &str) -> Self {
        Self(DynamicVec::from_slice(s.as_bytes()))
    }
}

impl From<&[u8]> for Bytes {
    #[inline]
    fn from(s: &[u8]) -> Self {
        Self(DynamicVec::from_slice(s))
    }
}

// `From<&[u8; N]>` lets `b"..."` byte-string literals (which have
// the array type `&'static [u8; N]`) flow through `impl Into<Bytes>`
// without an explicit `&...[..]` slicing dance at the call site.
impl<const N: usize> From<&[u8; N]> for Bytes {
    #[inline]
    fn from(s: &[u8; N]) -> Self {
        Self(DynamicVec::from_slice(s))
    }
}

impl From<String> for Bytes {
    #[inline]
    fn from(s: String) -> Self {
        let bytes = s.into_bytes();
        if bytes.len() <= INLINE_LIMIT {
            Self(DynamicVec::from_slice(&bytes))
        } else {
            Self(DynamicVec::from_eco(EcoVec::from_vec(bytes)))
        }
    }
}

impl From<Vec<u8>> for Bytes {
    #[inline]
    fn from(v: Vec<u8>) -> Self {
        if v.len() <= INLINE_LIMIT {
            Self(DynamicVec::from_slice(&v))
        } else {
            Self(DynamicVec::from_eco(EcoVec::from_vec(v)))
        }
    }
}

// --- Trait impls ---

impl Deref for Bytes {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &[u8] {
        self.0.as_slice()
    }
}

impl AsRef<[u8]> for Bytes {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        self
    }
}

impl Borrow<[u8]> for Bytes {
    #[inline]
    fn borrow(&self) -> &[u8] {
        self
    }
}

impl PartialEq for Bytes {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.as_ref() == other.as_ref()
    }
}

impl Eq for Bytes {}

impl PartialEq<[u8]> for Bytes {
    #[inline]
    fn eq(&self, other: &[u8]) -> bool {
        self.as_ref() == other
    }
}

impl PartialEq<&[u8]> for Bytes {
    #[inline]
    fn eq(&self, other: &&[u8]) -> bool {
        self.as_ref() == *other
    }
}

// `PartialEq<[u8; N]>` / `PartialEq<&[u8; N]>` let call sites
// compare a `Bytes` directly against a `b"..."` byte-string literal
// (which has the array type `&'static [u8; N]`) without an explicit
// `&...[..]` slicing dance.
impl<const N: usize> PartialEq<[u8; N]> for Bytes {
    #[inline]
    fn eq(&self, other: &[u8; N]) -> bool {
        self.as_ref() == other.as_slice()
    }
}

impl<const N: usize> PartialEq<&[u8; N]> for Bytes {
    #[inline]
    fn eq(&self, other: &&[u8; N]) -> bool {
        self.as_ref() == other.as_slice()
    }
}

impl PartialEq<str> for Bytes {
    #[inline]
    fn eq(&self, other: &str) -> bool {
        self.as_ref() == other.as_bytes()
    }
}

impl PartialEq<&str> for Bytes {
    #[inline]
    fn eq(&self, other: &&str) -> bool {
        self.as_ref() == other.as_bytes()
    }
}

impl PartialOrd for Bytes {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Bytes {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_ref().cmp(other.as_ref())
    }
}

impl Hash for Bytes {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_ref().hash(state);
    }
}

impl fmt::Debug for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(bstr::BStr::new(self.as_ref()), f)
    }
}

impl fmt::Display for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(bstr::BStr::new(self.as_ref()), f)
    }
}

// Safety: EcoVec uses atomic refcounting; InlineVec is Copy.
unsafe impl Send for Bytes {}
unsafe impl Sync for Bytes {}

impl Bytes {
    /// Returns `true` if this byte string is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ==========================================================================
// Internal: InlineVec
// ==========================================================================

const INLINE_LIMIT: usize = 23;
const LEN_TAG: u8 = 0b1000_0000;
const LEN_MASK: u8 = 0b0111_1111;

#[repr(C)]
#[derive(Copy, Clone)]
struct InlineVec {
    buf: [u8; INLINE_LIMIT],
    tagged_len: u8,
}

impl InlineVec {
    #[inline]
    const fn new() -> Self {
        Self {
            buf: [0; INLINE_LIMIT],
            tagged_len: LEN_TAG,
        }
    }

    #[inline]
    const fn from_slice(bytes: &[u8]) -> Result<Self, ()> {
        let len = bytes.len();
        if len > INLINE_LIMIT {
            return Err(());
        }
        let mut buf = [0u8; INLINE_LIMIT];
        let mut i = 0;
        while i < len {
            buf[i] = bytes[i];
            i += 1;
        }
        Ok(Self {
            buf,
            tagged_len: len as u8 | LEN_TAG,
        })
    }

    #[inline]
    fn len(&self) -> usize {
        usize::from(self.tagged_len & LEN_MASK)
    }

    #[inline]
    fn as_slice(&self) -> &[u8] {
        // Safety: invariant: len <= INLINE_LIMIT == buf.len()
        unsafe { self.buf.get_unchecked(..self.len()) }
    }
}

// ==========================================================================
// Internal: EcoVec<u8> — refcounted heap allocation
// ==========================================================================

/// Header stored at the start of every heap allocation.
#[repr(C)]
struct Header {
    refs: AtomicUsize,
    capacity: usize,
}

/// A reference-counted byte vector. Layout: ptr (to data) + len.
#[repr(C)]
struct EcoVec {
    /// Points `Self::offset()` bytes into the allocation (past the header).
    /// Equal to `Self::dangling()` when unallocated.
    ptr: NonNull<u8>,
    len: usize,
}

impl EcoVec {
    #[inline]
    const fn new() -> Self {
        Self {
            ptr: Self::dangling(),
            len: 0,
        }
    }

    fn with_capacity(capacity: usize) -> Self {
        let mut vec = Self::new();
        if capacity > 0 {
            // Safety: refcount starts at 1, capacity starts at 0
            unsafe { vec.grow(capacity) }
        }
        vec
    }

    /// Convert a `Vec<u8>` into an `EcoVec` by copying into our layout.
    fn from_vec(v: Vec<u8>) -> Self {
        let mut eco = Self::with_capacity(v.len());
        // Safety: we just allocated capacity >= v.len(), refcount is 1
        unsafe {
            ptr::copy_nonoverlapping(v.as_ptr(), eco.data_mut(), v.len());
            eco.len = v.len();
        }
        eco
    }

    #[inline]
    fn is_allocated(&self) -> bool {
        !ptr::eq(self.ptr.as_ptr(), Self::dangling().as_ptr())
    }

    #[inline]
    fn header(&self) -> Option<&Header> {
        self.is_allocated()
            .then(|| unsafe { &*self.allocation().cast::<Header>() })
    }

    #[inline]
    unsafe fn allocation(&self) -> *const u8 {
        debug_assert!(self.is_allocated());
        self.ptr.as_ptr().sub(Self::offset())
    }

    #[inline]
    unsafe fn allocation_mut(&mut self) -> *mut u8 {
        debug_assert!(self.is_allocated());
        self.ptr.as_ptr().sub(Self::offset())
    }

    #[inline]
    fn data(&self) -> *const u8 {
        self.ptr.as_ptr()
    }

    #[inline]
    unsafe fn data_mut(&mut self) -> *mut u8 {
        self.ptr.as_ptr()
    }

    fn capacity(&self) -> usize {
        self.header().map_or(0, |h| h.capacity)
    }

    fn as_slice(&self) -> &[u8] {
        // Safety: ptr is valid for len reads
        unsafe { core::slice::from_raw_parts(self.data(), self.len) }
    }

    /// Grow allocation to at least `target` capacity.
    /// May only be called when refcount == 1 and target > current capacity.
    unsafe fn grow(&mut self, target: usize) {
        debug_assert!(target > self.capacity());

        if target > isize::MAX as usize {
            capacity_overflow();
        }

        let layout = Self::layout(target);
        let alloc = if !self.is_allocated() {
            std::alloc::alloc(layout)
        } else {
            std::alloc::realloc(
                self.allocation_mut(),
                Self::layout(self.capacity()),
                Self::size(target),
            )
        };

        if alloc.is_null() {
            std::alloc::handle_alloc_error(layout);
        }

        self.ptr = NonNull::new_unchecked(alloc.add(Self::offset()));

        ptr::write(
            alloc.cast::<Header>(),
            Header {
                refs: AtomicUsize::new(1),
                capacity: target,
            },
        );
    }

    #[inline]
    fn layout(capacity: usize) -> Layout {
        // Safety: size and align are computed to be valid
        unsafe { Layout::from_size_align_unchecked(Self::size(capacity), Self::align()) }
    }

    #[inline]
    fn size(capacity: usize) -> usize {
        Self::offset()
            .checked_add(capacity)
            .filter(|&s| s < isize::MAX as usize - Self::align())
            .unwrap_or_else(|| capacity_overflow())
    }

    #[inline]
    const fn align() -> usize {
        mem::align_of::<Header>()
    }

    #[inline]
    const fn offset() -> usize {
        // Header size, rounded up to alignment of data (u8, so just header size)
        mem::size_of::<Header>()
    }

    #[inline]
    const fn dangling() -> NonNull<u8> {
        // Safety: offset() is always > 0
        unsafe { NonNull::new_unchecked(Self::offset() as *mut u8) }
    }
}

impl Clone for EcoVec {
    #[inline]
    fn clone(&self) -> Self {
        if let Some(header) = self.header() {
            let prev = header.refs.fetch_add(1, Relaxed);
            if prev > isize::MAX as usize {
                // Undo the increment before panicking
                header.refs.fetch_sub(1, Relaxed);
                panic!("reference count overflow");
            }
        }
        Self {
            ptr: self.ptr,
            len: self.len,
        }
    }
}

impl Drop for EcoVec {
    #[inline(always)]
    fn drop(&mut self) {
        if self
            .header()
            .map_or(true, |h| h.refs.fetch_sub(1, Release) != 1)
        {
            return;
        }
        atomic::fence(Acquire);

        // Safety: we are the last reference
        unsafe {
            std::alloc::dealloc(self.allocation_mut(), Self::layout(self.capacity()));
        }
    }
}

// Safety: atomic refcount
unsafe impl Send for EcoVec {}
unsafe impl Sync for EcoVec {}

// ==========================================================================
// Internal: DynamicVec — union of InlineVec and EcoVec
// ==========================================================================

#[repr(C)]
union Repr {
    inline: InlineVec,
    spilled: ManuallyDrop<EcoVec>,
}

struct DynamicVec(Repr);

impl DynamicVec {
    #[inline]
    const fn new() -> Self {
        Self(Repr {
            inline: InlineVec::new(),
        })
    }

    #[inline]
    fn from_inline(inline: InlineVec) -> Self {
        Self(Repr { inline })
    }

    #[inline]
    fn from_eco(vec: EcoVec) -> Self {
        // We must ensure tagged_len = 0 so is_inline() returns false.
        let mut repr = Repr {
            inline: InlineVec {
                buf: [0; INLINE_LIMIT],
                tagged_len: 0,
            },
        };
        repr.spilled = ManuallyDrop::new(vec);
        Self(repr)
    }

    #[inline]
    fn from_slice(bytes: &[u8]) -> Self {
        match InlineVec::from_slice(bytes) {
            Ok(inline) => Self::from_inline(inline),
            Err(()) => Self::from_eco(EcoVec::from_vec(bytes.to_vec())),
        }
    }

    #[inline]
    fn is_inline(&self) -> bool {
        // Safety: tagged_len is always initialized for both variants
        unsafe { self.0.inline.tagged_len & LEN_TAG != 0 }
    }

    #[inline]
    fn as_slice(&self) -> &[u8] {
        if self.is_inline() {
            unsafe { self.0.inline.as_slice() }
        } else {
            unsafe { self.0.spilled.as_slice() }
        }
    }
}

impl Clone for DynamicVec {
    #[inline]
    fn clone(&self) -> Self {
        if self.is_inline() {
            // Safety: we just checked
            Self::from_inline(unsafe { self.0.inline })
        } else {
            // Safety: we just checked
            Self::from_eco(unsafe { (*self.0.spilled).clone() })
        }
    }
}

impl Drop for DynamicVec {
    #[inline(always)]
    fn drop(&mut self) {
        if !self.is_inline() {
            // Safety: we just checked that it's spilled
            unsafe {
                ptr::drop_in_place(&mut *self.0.spilled);
            }
        }
    }
}

#[cold]
fn capacity_overflow() -> ! {
    panic!("capacity overflow");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_of_bytes_is_24() {
        k9::assert_equal!(mem::size_of::<Bytes>(), 24);
    }

    #[test]
    fn empty_bytes() {
        let b = Bytes::default();
        k9::assert_equal!(b.len(), 0);
        k9::assert_equal!(b.is_empty(), true);
        k9::assert_equal!(b.as_ref(), &[] as &[u8]);
    }

    #[test]
    fn from_short_str() {
        let b = Bytes::from("hello");
        k9::assert_equal!(b.len(), 5);
        k9::assert_equal!(b.as_ref(), b"hello" as &[u8]);
    }

    #[test]
    fn from_23_byte_str_is_inline() {
        let s = "12345678901234567890123"; // exactly 23 bytes
        let b = Bytes::from(s);
        k9::assert_equal!(b.len(), 23);
        k9::assert_equal!(b.as_ref(), s.as_bytes());
        assert!(b.0.is_inline());
    }

    #[test]
    fn from_24_byte_str_spills() {
        let s = "123456789012345678901234"; // 24 bytes
        let b = Bytes::from(s);
        k9::assert_equal!(b.len(), 24);
        k9::assert_equal!(b.as_ref(), s.as_bytes());
        assert!(!b.0.is_inline());
    }

    #[test]
    fn clone_inline_is_independent() {
        let a = Bytes::from("hello");
        let b = a.clone();
        k9::assert_equal!(a, b);
        k9::assert_equal!(a.as_ref(), b.as_ref());
    }

    #[test]
    fn clone_spilled_shares_allocation() {
        let a = Bytes::from("this is a longer string that spills to heap");
        let b = a.clone();
        k9::assert_equal!(a, b);
        // Both should point to the same data
        k9::assert_equal!(a.as_ptr(), b.as_ptr());
    }

    #[test]
    fn from_byte_slice() {
        let data: &[u8] = &[0, 1, 2, 3, 255];
        let b = Bytes::from(data);
        k9::assert_equal!(b.as_ref(), data);
    }

    #[test]
    fn from_string_short() {
        let s = String::from("hi");
        let b = Bytes::from(s);
        k9::assert_equal!(b.as_ref(), b"hi" as &[u8]);
        assert!(b.0.is_inline());
    }

    #[test]
    fn from_string_long() {
        let s = String::from("this string is definitely longer than fifteen bytes");
        let expected = s.as_bytes().to_vec();
        let b = Bytes::from(s);
        k9::assert_equal!(b.as_ref(), expected.as_slice());
        assert!(!b.0.is_inline());
    }

    #[test]
    fn from_vec_short() {
        let v = vec![1u8, 2, 3];
        let b = Bytes::from(v);
        k9::assert_equal!(b.as_ref(), [1u8, 2, 3].as_slice());
        assert!(b.0.is_inline());
    }

    #[test]
    fn from_vec_long() {
        let v: Vec<u8> = (0..32).collect();
        let expected = v.clone();
        let b = Bytes::from(v);
        k9::assert_equal!(b.as_ref(), expected.as_slice());
        assert!(!b.0.is_inline());
    }

    #[test]
    fn equality() {
        let a = Bytes::from("hello");
        let b = Bytes::from("hello");
        let c = Bytes::from("world");
        k9::assert_equal!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn ordering() {
        let a = Bytes::from("aaa");
        let b = Bytes::from("bbb");
        assert!(a < b);
        assert!(b > a);
        k9::assert_equal!(a.cmp(&a), Ordering::Equal);
    }

    #[test]
    fn hashing_consistent() {
        use std::collections::hash_map::DefaultHasher;

        fn hash_of(b: &Bytes) -> u64 {
            let mut h = DefaultHasher::new();
            b.hash(&mut h);
            h.finish()
        }

        let a = Bytes::from("test");
        let b = Bytes::from("test");
        k9::assert_equal!(hash_of(&a), hash_of(&b));

        // Hash should match hashing the raw slice
        let mut h1 = DefaultHasher::new();
        a.hash(&mut h1);
        let mut h2 = DefaultHasher::new();
        b"test".as_slice().hash(&mut h2);
        k9::assert_equal!(h1.finish(), h2.finish());
    }

    #[test]
    fn debug_display() {
        let b = Bytes::from("hello");
        k9::assert_equal!(format!("{b:?}"), "\"hello\"");
        k9::assert_equal!(format!("{b}"), "hello");
    }

    #[test]
    fn debug_display_non_utf8() {
        let b = Bytes::from(&[0xff, 0xfe][..]);
        // bstr renders invalid UTF-8 with escapes
        let dbg = format!("{b:?}");
        assert!(dbg.len() > 0);
    }

    #[test]
    fn partial_eq_with_slices() {
        let b = Bytes::from("hello");
        assert!(b == b"hello"[..]);
        assert!(b == "hello");
    }

    #[test]
    fn drop_spilled_does_not_leak() {
        // Just exercise the drop path for spilled allocations
        let b = Bytes::from("a long string that definitely spills to heap allocation");
        let c = b.clone();
        drop(b);
        k9::assert_equal!(
            c.as_ref(),
            b"a long string that definitely spills to heap allocation" as &[u8]
        );
        drop(c);
    }

    #[test]
    fn many_clones_and_drops() {
        let original = Bytes::from("shared heap string that is long enough");
        let clones: Vec<_> = (0..100).map(|_| original.clone()).collect();
        drop(original);
        for c in &clones {
            k9::assert_equal!(
                c.as_ref(),
                b"shared heap string that is long enough" as &[u8]
            );
        }
        drop(clones);
    }

    #[test]
    fn send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Bytes>();
    }

    #[test]
    fn deref_indexing() {
        let b = Bytes::from("hello");
        k9::assert_equal!(b[0], b'h');
        k9::assert_equal!(b[4], b'o');
        k9::assert_equal!(&b[1..4], b"ell" as &[u8]);
    }

    #[test]
    fn empty_from_empty_str() {
        let b = Bytes::from("");
        k9::assert_equal!(b.is_empty(), true);
        k9::assert_equal!(b.len(), 0);
        assert!(b.0.is_inline());
    }

    #[test]
    fn empty_from_empty_slice() {
        let b = Bytes::from(&[][..]);
        k9::assert_equal!(b.is_empty(), true);
        assert!(b.0.is_inline());
    }

    #[test]
    fn boundary_14_bytes() {
        let s = "12345678901234"; // 14 bytes
        let b = Bytes::from(s);
        k9::assert_equal!(b.len(), 14);
        assert!(b.0.is_inline());
        k9::assert_equal!(b.as_ref(), s.as_bytes());
    }

    #[test]
    fn from_vec_empty() {
        let b = Bytes::from(Vec::<u8>::new());
        k9::assert_equal!(b.is_empty(), true);
        assert!(b.0.is_inline());
    }

    #[test]
    fn from_string_empty() {
        let b = Bytes::from(String::new());
        k9::assert_equal!(b.is_empty(), true);
        assert!(b.0.is_inline());
    }
}
