// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    clippy::missing_panics_doc,
    clippy::undocumented_unsafe_blocks,
    reason = "reachable panics are programmer error, and unsafe code follows the documented `data`/`len`/`cap` invariants"
)]

use core::alloc::Layout;
use core::ops::{Bound, RangeBounds};
use core::ptr::{NonNull, copy_nonoverlapping};

use allocator_api2::alloc::{AllocError, Allocator, Global};

use crate::Arena;
use crate::arena::panic_alloc;
use crate::strings::RcStr;

/// Initial capacity granted on the first growth (when none was specified).
const MIN_INITIAL_CAP: usize = 16;

/// A growable, mutable UTF-8 string that lives in an [`Arena`].
///
/// `String` is a **transient builder**: 32 bytes on 64-bit (data pointer +
/// length + capacity + arena reference). Its purpose is to be filled and
/// then frozen via [`Self::into_arena_str`] into a compact, immutable
/// [`RcStr`] (8 bytes). The freeze is **O(1)** — no copy, no new
/// allocation.
///
/// # Example
///
/// ```
/// use multitude::Arena;
///
/// let arena = Arena::new();
/// let mut s = arena.alloc_string();
/// s.push_str("hello, ");
/// s.push_str("world!");
/// assert_eq!(s.as_str(), "hello, world!");
/// let frozen = s.into_arena_str(); // O(1), no copy
/// assert_eq!(&*frozen, "hello, world!");
/// ```
pub struct String<'a, A: Allocator + Clone = Global> {
    arena: &'a Arena<A>,
    /// Data pointer, just past the inline length prefix.
    /// Dangling when `cap == 0`.
    data: NonNull<u8>,
    len: usize,
    cap: usize,
}

impl<'a, A: Allocator + Clone> String<'a, A> {
    /// Create a new, empty arena-backed string.
    ///
    /// No allocation is performed until the first push.
    #[must_use]
    pub const fn new_in(arena: &'a Arena<A>) -> Self {
        Self {
            arena,
            data: NonNull::dangling(),
            len: 0,
            cap: 0,
        }
    }

    /// Create a new arena-backed string with at least `cap` bytes of
    /// pre-allocated capacity.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails. Use [`Self::try_with_capacity_in`] for a
    /// fallible variant.
    #[must_use]
    pub fn with_capacity_in(cap: usize, arena: &'a Arena<A>) -> Self {
        crate::arena::expect_alloc(Self::try_with_capacity_in(cap, arena))
    }

    /// Fallible variant of [`Self::with_capacity_in`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`](allocator_api2::alloc::AllocError) if the backing
    /// allocator fails.
    pub fn try_with_capacity_in(cap: usize, arena: &'a Arena<A>) -> Result<Self, AllocError> {
        let mut string = Self::new_in(arena);
        if cap > 0 {
            string.try_allocate_initial(cap)?;
        }
        Ok(string)
    }

    /// Returns the length of this string in bytes.
    #[must_use]
    #[inline]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if this string has a length of zero.
    #[must_use]
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns this string's capacity, in bytes.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.cap
    }

    /// Returns a string slice view of this string's contents.
    #[must_use]
    pub fn as_str(&self) -> &str {
        if self.len == 0 {
            return "";
        }
        // SAFETY: all mutation APIs append, remove, and shift only valid UTF-8 at character boundaries.
        unsafe { core::str::from_utf8_unchecked(self.as_bytes()) }
    }

    /// Return the bytes view of this string.
    #[must_use]
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        if self.len == 0 {
            return &[];
        }
        // SAFETY: `cap > 0` when `len > 0`; the first `len` bytes are initialized string data.
        unsafe { core::slice::from_raw_parts(self.data.as_ptr(), self.len) }
    }

    /// Return a mutable `str` view of this string.
    ///
    /// Callers must preserve UTF-8 well-formedness; mutating the bytes
    /// in a way that produces invalid UTF-8 is undefined behavior, but
    /// only via the unsafe `str` APIs that allow byte-level edits.
    #[inline]
    pub fn as_mut_str(&mut self) -> &mut str {
        if self.len == 0 {
            // SAFETY: a zero-length slice may be formed from a dangling non-null pointer, and empty bytes are valid UTF-8.
            return unsafe { core::str::from_utf8_unchecked_mut(core::slice::from_raw_parts_mut(self.data.as_ptr(), 0)) };
        }
        // SAFETY: `cap > 0` when `len > 0`; the first `len` bytes are initialized valid UTF-8.
        unsafe { core::str::from_utf8_unchecked_mut(core::slice::from_raw_parts_mut(self.data.as_ptr(), self.len)) }
    }

    /// Return a raw const pointer to the string's bytes.
    #[must_use]
    #[inline]
    pub const fn as_ptr(&self) -> *const u8 {
        self.data.as_ptr()
    }

    /// Return a raw mutable pointer to the string's bytes.
    #[expect(clippy::needless_pass_by_ref_mut, reason = "API shape mirrors std::String::as_mut_ptr")]
    #[inline]
    pub const fn as_mut_ptr(&mut self) -> *mut u8 {
        self.data.as_ptr()
    }

    /// Construct a `String` containing `s`, copied into `arena`.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails.
    #[must_use]
    pub fn from_str_in(s: &str, arena: &'a Arena<A>) -> Self {
        let mut string = Self::new_in(arena);
        string.push_str(s);
        string
    }

    /// Remove the last character from the string and return it.
    ///
    /// Returns `None` if the string is empty.
    pub fn pop(&mut self) -> Option<char> {
        let ch = self.as_str().chars().next_back()?;
        self.len -= ch.len_utf8();
        Some(ch)
    }

    /// Shorten the string to `new_len` bytes.
    ///
    /// If `new_len >= self.len()`, this has no effect. Capacity is
    /// unchanged.
    ///
    /// # Panics
    ///
    /// Panics if `new_len` is not on a UTF-8 character boundary.
    pub fn truncate(&mut self, new_len: usize) {
        if new_len >= self.len {
            return;
        }
        assert!(self.as_str().is_char_boundary(new_len), "truncate index is not a char boundary");
        self.len = new_len;
    }

    /// Try to release any unused capacity back to the chunk's bump
    /// cursor.
    ///
    /// Only succeeds if this string's buffer ends exactly at the
    /// chunk's bump cursor (no later allocations have been made into
    /// the same chunk). On failure this is a no-op.
    #[cfg_attr(test, mutants::skip)] // Full-capacity shrink mutates to a zero-byte cursor reclaim.
    pub fn shrink_to_fit(&mut self) {
        if self.cap == 0 || self.len == self.cap {
            return;
        }
        let reclaim = self.cap - self.len;
        let buffer_end = unsafe { self.data.as_ptr().add(self.cap) };
        // SAFETY: buffer_end is data + cap, within the same chunk payload.
        if unsafe { self.arena.try_shrink_at_cursor(buffer_end, reclaim) } {
            self.cap = self.len;
        }
    }

    /// Insert a character at byte index `idx`.
    ///
    /// # Panics
    ///
    /// Panics if `idx` is greater than `self.len()` or not on a UTF-8
    /// character boundary, or if the backing allocator fails on growth.
    pub fn insert(&mut self, idx: usize, ch: char) {
        let mut buf = [0_u8; 4];
        self.insert_str(idx, ch.encode_utf8(&mut buf));
    }

    /// Insert a string slice at byte index `idx`.
    ///
    /// # Panics
    ///
    /// Panics if `idx` is greater than `self.len()`, if `idx` is not
    /// on a UTF-8 character boundary, if the resulting length would
    /// overflow `usize`, or if the backing allocator fails on growth.
    #[cfg_attr(test, mutants::skip)] // Copy-count mutations are only distinguishable by UB tooling.
    pub fn insert_str(&mut self, idx: usize, s: &str) {
        assert!(idx <= self.len, "insertion index out of bounds");
        assert!(self.as_str().is_char_boundary(idx), "insertion index is not a char boundary");
        let new_len = self.len.checked_add(s.len()).expect("inserted string length must fit in usize");
        if new_len > self.cap {
            self.grow_to_at_least(new_len);
        }
        // SAFETY: `idx <= len <= cap`, growth above guarantees `new_len <= cap`, and `ptr::copy` handles overlap.
        unsafe {
            let ptr = self.data.as_ptr().add(idx);
            core::ptr::copy(ptr, ptr.add(s.len()), self.len - idx);
            copy_nonoverlapping(s.as_ptr(), ptr, s.len());
        }
        self.len = new_len;
    }

    /// Remove the character at byte index `idx` and return it.
    ///
    /// # Panics
    ///
    /// Panics if `idx >= self.len()` or `idx` is not on a UTF-8
    /// character boundary.
    #[cfg_attr(test, mutants::skip)] // Copy-count mutations are only distinguishable by UB tooling.
    pub fn remove(&mut self, idx: usize) -> char {
        assert!(idx < self.len, "removal index out of bounds");
        assert!(self.as_str().is_char_boundary(idx), "removal index is not a char boundary");
        let ch = self.as_str()[idx..]
            .chars()
            .next()
            .expect("idx is in bounds and on a char boundary");
        let ch_len = ch.len_utf8();
        // SAFETY: `idx + ch_len <= len`; overlapping copy shifts the initialized tail left.
        unsafe {
            let ptr = self.data.as_ptr().add(idx);
            core::ptr::copy(ptr.add(ch_len), ptr, self.len - idx - ch_len);
        }
        self.len -= ch_len;
        ch
    }

    /// Retain only the characters for which `f` returns `true`, in
    /// order.
    ///
    /// If `f` panics, any not-yet-processed characters are dropped from
    /// the string (matching `std::string::String::retain`).
    #[cfg_attr(test, mutants::skip)] // Slice-length mutation does not affect the next-char read.
    pub fn retain<F: FnMut(char) -> bool>(&mut self, mut f: F) {
        struct Guard<'s, 'a, A: Allocator + Clone> {
            string: &'s mut String<'a, A>,
            len: usize,
            active: bool,
        }

        impl<A: Allocator + Clone> Drop for Guard<'_, '_, A> {
            fn drop(&mut self) {
                if self.active {
                    self.string.len = self.len;
                }
            }
        }

        let old_len = self.len;
        let mut read = 0;
        let mut write = 0;
        let data = self.data;
        let mut guard = Guard {
            string: self,
            len: 0,
            active: true,
        };

        while read < old_len {
            let ch;
            let ch_len;
            {
                // End the borrowed tail before the later raw write.
                // SAFETY: `read` is advanced by UTF-8 character lengths and remains within the original initialized string.
                let tail = unsafe { core::slice::from_raw_parts(data.as_ptr().add(read), old_len - read) };
                // SAFETY: `tail` starts on a character boundary inside the original valid UTF-8 string.
                ch = unsafe { core::str::from_utf8_unchecked(tail) }
                    .chars()
                    .next()
                    .expect("read cursor remains before original string end");
                ch_len = ch.len_utf8();
            }
            guard.len = write;
            if f(ch) {
                if write != read {
                    // SAFETY: source and destination are within the same allocation; `ptr::copy` handles overlap.
                    unsafe { core::ptr::copy(data.as_ptr().add(read), data.as_ptr().add(write), ch_len) };
                }
                write += ch_len;
                guard.len = write;
            }
            read += ch_len;
        }
        guard.string.len = write;
        guard.active = false;
    }

    /// Replace the bytes in `range` with the contents of `replace_with`.
    ///
    /// # Panics
    ///
    /// Panics if either bound is out of range, the bounds are not on
    /// UTF-8 character boundaries, the resulting length would overflow
    /// `usize`, or the backing allocator fails on growth.
    #[cfg_attr(test, mutants::skip)] // Copy-count mutations are only distinguishable by UB tooling.
    pub fn replace_range<R: RangeBounds<usize>>(&mut self, range: R, replace_with: &str) {
        let start = match range.start_bound() {
            Bound::Included(&n) => n,
            Bound::Excluded(&n) => n
                .checked_add(1)
                .expect("excluded replace_range start bound must not overflow usize"),
            Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            Bound::Included(&n) => n.checked_add(1).expect("included replace_range end bound must not overflow usize"),
            Bound::Excluded(&n) => n,
            Bound::Unbounded => self.len,
        };
        assert!(start <= end, "replace_range start exceeds end");
        assert!(end <= self.len, "replace_range end out of bounds");
        assert!(self.as_str().is_char_boundary(start), "replace_range start is not a char boundary");
        assert!(self.as_str().is_char_boundary(end), "replace_range end is not a char boundary");

        let removed = end - start;
        let new_len = self
            .len
            .checked_sub(removed)
            .and_then(|base| base.checked_add(replace_with.len()))
            .expect("replacement string length must fit in usize");
        if new_len > self.cap {
            self.grow_to_at_least(new_len);
        }
        // SAFETY: bounds are in range and on character boundaries; growth above guarantees the final length fits capacity.
        unsafe {
            let ptr = self.data.as_ptr();
            if replace_with.len() != removed {
                core::ptr::copy(ptr.add(end), ptr.add(start + replace_with.len()), self.len - end);
            }
            copy_nonoverlapping(replace_with.as_ptr(), ptr.add(start), replace_with.len());
        }
        self.len = new_len;
    }

    /// Append a single character.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails. Use [`Self::try_push`] for a fallible
    /// variant.
    #[inline]
    pub fn push(&mut self, ch: char) {
        if self.try_push(ch).is_err() {
            panic_alloc();
        }
    }

    /// Fallible variant of [`Self::push`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`](allocator_api2::alloc::AllocError) if the backing
    /// allocator fails.
    #[inline]
    pub fn try_push(&mut self, ch: char) -> Result<(), AllocError> {
        let mut buf = [0_u8; 4];
        self.try_push_str(ch.encode_utf8(&mut buf))
    }

    /// Append a string slice.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails. Use [`Self::try_push_str`] for a fallible
    /// variant.
    #[inline]
    pub fn push_str(&mut self, s: impl AsRef<str>) {
        if self.try_push_str(s).is_err() {
            panic_alloc();
        }
    }

    /// Fallible variant of [`Self::push_str`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`](allocator_api2::alloc::AllocError) if the backing
    /// allocator fails.
    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "the hot path through `try_push_str` is the bump-then-memcpy fast path; the cold grow branch is `#[inline(never)]` so the inlinable body is small"
    )]
    pub fn try_push_str(&mut self, s: impl AsRef<str>) -> Result<(), AllocError> {
        let s = s.as_ref();
        let needed = self.len.checked_add(s.len()).ok_or(AllocError)?;
        if needed > self.cap {
            self.try_grow_to_at_least(needed)?;
        }
        // SAFETY: growth above guarantees enough tail capacity; source is a valid `str` byte slice.
        unsafe { copy_nonoverlapping(s.as_ptr(), self.data.as_ptr().add(self.len), s.len()) };
        self.len = needed;
        Ok(())
    }

    /// Reserve capacity for at least `additional` more bytes.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails. Use [`Self::try_reserve`] for a fallible
    /// variant.
    #[inline]
    pub fn reserve(&mut self, additional: usize) {
        if self.try_reserve(additional).is_err() {
            panic_alloc();
        }
    }

    /// Fallible variant of [`Self::reserve`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`](allocator_api2::alloc::AllocError) if the backing
    /// allocator fails.
    #[inline]
    pub fn try_reserve(&mut self, additional: usize) -> Result<(), AllocError> {
        let needed = self.len.checked_add(additional).ok_or(AllocError)?;
        if needed > self.cap {
            self.try_grow_to_at_least(needed)?;
        }
        Ok(())
    }

    /// Truncates this string, removing all contents.
    ///
    /// The capacity is preserved.
    pub const fn clear(&mut self) {
        self.len = 0;
    }

    /// Freeze into an immutable [`RcStr<A>`].
    ///
    /// **O(1)** — the length is already stored inline at the right
    /// position, so this just transfers the data pointer.
    ///
    /// If the buffer sits at the chunk's bump cursor (the common case
    /// when no other allocations happened during the build), any unused
    /// capacity is reclaimed back to the cursor so subsequent
    /// allocations can reuse those bytes.
    #[must_use]
    pub fn into_arena_str(self) -> RcStr<A> {
        if self.cap == 0 {
            let arena = self.arena;
            core::mem::forget(self);
            return arena.alloc_str_rc("");
        }
        // SAFETY: `cap > 0` and all mutation methods maintain `len <= cap`.
        unsafe { self.flush_len_to_prefix() };
        self.try_reclaim_tail();
        let data = self.data;
        core::mem::forget(self);
        // SAFETY: `data` points at valid UTF-8 bytes with an initialized inline length prefix and one chunk refcount.
        unsafe { RcStr::from_raw_data(data) }
    }

    /// Freeze into an owned, mutable [`BoxStr<A>`](crate::strings::BoxStr).
    ///
    /// **O(1)** — same shape as [`Self::into_arena_str`], but produces
    /// an owned [`BoxStr`](crate::strings::BoxStr) whose `Drop`
    /// releases the chunk hold immediately.
    #[must_use]
    pub fn into_arena_box_str(self) -> crate::strings::BoxStr<A> {
        if self.cap == 0 {
            let arena = self.arena;
            core::mem::forget(self);
            return arena.alloc_str_box("");
        }
        // SAFETY: `cap > 0` and all mutation methods maintain `len <= cap`.
        unsafe { self.flush_len_to_prefix() };
        self.try_reclaim_tail();
        let data = self.data;
        core::mem::forget(self);
        // SAFETY: `data` points at valid UTF-8 bytes with an initialized inline length prefix and one chunk refcount.
        unsafe { crate::strings::BoxStr::from_raw_data(data) }
    }

    /// Best-effort reclaim of unused tail bytes.
    #[inline]
    fn try_reclaim_tail(&self) {
        if self.len >= self.cap {
            return;
        }
        let prefix_size = core::mem::size_of::<usize>();
        let total = prefix_size + self.cap;
        let used = prefix_size + self.len;
        let reclaim = total - used;
        // SAFETY: `data + cap` is one past the buffer end.
        let buffer_end = unsafe { self.data.as_ptr().add(self.cap) };
        // SAFETY: `buffer_end` and `reclaim` describe the unused tail of this allocation.
        let _ = unsafe { self.arena.try_shrink_at_cursor(buffer_end, reclaim) };
    }

    /// Write `self.len` to the inline prefix slot in the chunk.
    ///
    /// # Safety
    ///
    /// Requires `self.cap > 0` and `self.len <= self.cap`.
    #[inline]
    #[allow(
        clippy::cast_ptr_alignment,
        reason = "prefix slot is accessed via write_unaligned; alignment of the cast target is not relied on"
    )]
    unsafe fn flush_len_to_prefix(&self) {
        let prefix_size = core::mem::size_of::<usize>();
        // SAFETY: caller guarantees allocation happened, so `data`
        // points immediately after the prefix.
        let prefix_ptr = unsafe { self.data.as_ptr().sub(prefix_size).cast::<usize>() };
        // SAFETY: this string exclusively owns the prefix slot and still owns
        // the allocation's chunk ref.
        let slot = unsafe { crate::internal::slot::UninitSlot::<usize>::from_raw(prefix_ptr) };
        slot.write_unaligned(self.len);
    }

    #[cold]
    #[inline(never)]
    fn try_allocate_initial(&mut self, cap: usize) -> Result<(), AllocError> {
        let prefix_size = core::mem::size_of::<usize>();
        let total = prefix_size.checked_add(cap).ok_or(AllocError)?;
        // Byte alignment is enough: the prefix is accessed unaligned and the
        // payload is `u8`.
        let layout = Layout::from_size_align(total, core::mem::align_of::<u8>()).map_err(|_e| AllocError)?;
        let ptr = self.arena.allocate_layout(layout)?;
        // SAFETY: `ptr` is non-null and `total >= prefix_size`, so adding `prefix_size` stays within or one past the allocation.
        self.data = unsafe { NonNull::new_unchecked(ptr.as_ptr().add(prefix_size)) };
        self.cap = cap;
        Ok(())
    }

    #[cold]
    #[inline(never)]
    fn try_grow_to_at_least(&mut self, min_cap: usize) -> Result<(), AllocError> {
        debug_assert!(min_cap > self.cap, "callers must gate the grow");
        let doubled = self.cap.saturating_mul(2);
        let new_cap = core::cmp::max(min_cap, core::cmp::max(doubled, MIN_INITIAL_CAP));
        if self.cap == 0 {
            return self.try_allocate_initial(new_cap);
        }

        let prefix_size = core::mem::size_of::<usize>();
        let old_total = prefix_size.checked_add(self.cap).ok_or(AllocError)?;
        let new_total = prefix_size.checked_add(new_cap).ok_or(AllocError)?;
        // SAFETY: `cap > 0`, so `data` points immediately after an allocated prefix.
        let prefix_ptr = unsafe { NonNull::new_unchecked(self.data.as_ptr().sub(prefix_size)) };
        // SAFETY: `prefix_ptr` and `old_total` describe the existing string allocation; `new_total >= old_total`.
        let new_prefix = unsafe {
            self.arena
                .grow_for_string(prefix_ptr, old_total, new_total, core::mem::align_of::<u8>())?
        };
        let relocated = new_prefix != prefix_ptr;
        // Publish the new buffer before releasing the old chunk ref so an
        // unwind during release cannot make `Drop` see freed memory.
        // SAFETY: `new_prefix` is non-null and the prefix is followed by `new_cap` data bytes.
        self.data = unsafe { NonNull::new_unchecked(new_prefix.as_ptr().add(prefix_size)) };
        self.cap = new_cap;
        if relocated {
            // SAFETY: prefix_ptr is in a local chunk and the old
            // allocation's +1 belongs to this string.
            let in_chunk = unsafe { crate::internal::in_chunk::InLocalChunk::<_, A>::new(prefix_ptr) };
            let chunk = in_chunk.chunk_ptr();
            // SAFETY: the old allocation's chunk refcount is positive and is no longer owned by this string after relocation.
            unsafe { crate::internal::local_chunk::LocalChunk::dec_ref(chunk) };
        }
        Ok(())
    }

    fn grow_to_at_least(&mut self, min_cap: usize) {
        if self.try_grow_to_at_least(min_cap).is_err() {
            panic_alloc();
        }
    }
}

crate::arena_str_macros::impl_arena_string_forwarding_traits!(String, 'a, str, as_str, as_mut_str);

impl<A: Allocator + Clone> Clone for String<'_, A> {
    fn clone(&self) -> Self {
        Self::from_str_in(self.as_str(), self.arena)
    }
}

impl<A: Allocator + Clone> Extend<char> for String<'_, A> {
    fn extend<I: IntoIterator<Item = char>>(&mut self, iter: I) {
        for ch in iter {
            self.push(ch);
        }
    }
}

impl<'a, A: Allocator + Clone> Extend<&'a str> for String<'_, A> {
    fn extend<I: IntoIterator<Item = &'a str>>(&mut self, iter: I) {
        for s in iter {
            self.push_str(s);
        }
    }
}
#[cfg(feature = "serde")]
#[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
impl<A: Allocator + Clone> serde::ser::Serialize for String<'_, A> {
    fn serialize<S: serde::ser::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}
impl<'a, A: Allocator + Clone> crate::vec::FromIteratorIn<char> for String<'a, A> {
    type Allocator = &'a Arena<A>;

    fn from_iter_in<I: IntoIterator<Item = char>>(iter: I, allocator: &'a Arena<A>) -> Self {
        let mut string = String::new_in(allocator);
        string.extend(iter);
        string
    }
}

impl<'a, 'b, A: Allocator + Clone> crate::vec::FromIteratorIn<&'b str> for String<'a, A> {
    type Allocator = &'a Arena<A>;

    fn from_iter_in<I: IntoIterator<Item = &'b str>>(iter: I, allocator: &'a Arena<A>) -> Self {
        let mut string = String::new_in(allocator);
        string.extend(iter);
        string
    }
}
