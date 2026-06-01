// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    clippy::cast_ptr_alignment,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::undocumented_unsafe_blocks,
    reason = "pointer casts target allocated prefix or payload slots; internal docs mirror std-style APIs; unsafe code follows the documented `data`/`len`/`cap` invariants"
)]

use core::alloc::Layout;
use core::ptr::{NonNull, copy_nonoverlapping};

use allocator_api2::alloc::{AllocError, Allocator, Global};
use widestring::Utf16Str;

use crate::Arena;
use crate::arena::panic_alloc;
use crate::strings::RcUtf16Str;

/// Initial capacity (in `u16` elements) granted on the first growth
/// when none was specified.
const MIN_INITIAL_CAP: usize = 16;

/// A growable, mutable UTF-16 string that lives in an [`Arena`].
///
/// `Utf16String` is a **transient builder**: 32 bytes on 64-bit (data
/// pointer + length + capacity + arena reference). Lengths and
/// capacities are counted in `u16` elements.
///
/// # Example
///
/// ```
/// # #[cfg(feature = "utf16")] {
/// use multitude::Arena;
/// use widestring::utf16str;
///
/// let arena = Arena::new();
/// let mut s = arena.alloc_utf16_string();
/// s.push_str(utf16str!("hello, "));
/// s.push_str(utf16str!("world!"));
/// assert_eq!(s.as_utf16_str(), utf16str!("hello, world!"));
/// let frozen = s.into_arena_utf16_str();
/// assert_eq!(&*frozen, utf16str!("hello, world!"));
/// # }
/// ```
pub struct Utf16String<'a, A: Allocator + Clone = Global> {
    arena: &'a Arena<A>,
    /// Data pointer, just past the inline length prefix.
    /// Dangling when `cap == 0`.
    data: NonNull<u16>,
    /// Length in `u16`s.
    len: usize,
    /// Capacity in `u16` elements.
    cap: usize,
}

impl<'a, A: Allocator + Clone> Utf16String<'a, A> {
    /// Create a new, empty arena-backed UTF-16 string.
    #[must_use]
    pub const fn new_in(arena: &'a Arena<A>) -> Self {
        Self {
            arena,
            data: NonNull::dangling(),
            len: 0,
            cap: 0,
        }
    }

    /// Create a new arena-backed UTF-16 string with at least `cap` `u16`
    /// elements of pre-allocated capacity.
    #[must_use]
    pub fn with_capacity_in(cap: usize, arena: &'a Arena<A>) -> Self {
        crate::arena::expect_alloc(Self::try_with_capacity_in(cap, arena))
    }

    /// Fallible variant of [`Self::with_capacity_in`].
    pub fn try_with_capacity_in(cap: usize, arena: &'a Arena<A>) -> Result<Self, AllocError> {
        let mut s = Self::new_in(arena);
        if cap > 0 {
            s.try_allocate_initial(cap)?;
        }
        Ok(s)
    }

    /// String length in `u16` elements.
    #[must_use]
    #[inline]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// True iff the string is empty.
    #[must_use]
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Capacity in `u16` elements.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.cap
    }

    /// Borrow as `&Utf16Str`.
    #[must_use]
    pub fn as_utf16_str(&self) -> &Utf16Str {
        // SAFETY: all mutators preserve UTF-16 well-formedness.
        unsafe { Utf16Str::from_slice_unchecked(self.as_slice()) }
    }

    /// Return the `u16` slice view.
    #[must_use]
    #[inline]
    pub fn as_slice(&self) -> &[u16] {
        if self.len == 0 {
            return &[];
        }
        // SAFETY: `cap > 0` when `len > 0`; first `len` elements are initialized.
        unsafe { core::slice::from_raw_parts(self.data.as_ptr(), self.len) }
    }

    /// Return a mutable `Utf16Str` view of this string.
    #[inline]
    pub fn as_mut_utf16_str(&mut self) -> &mut Utf16Str {
        if self.len == 0 {
            // SAFETY: empty slice may be formed from dangling non-null.
            let slice = unsafe { core::slice::from_raw_parts_mut(self.data.as_ptr(), 0) };
            // SAFETY: empty UTF-16 is valid.
            return unsafe { Utf16Str::from_slice_unchecked_mut(slice) };
        }
        // SAFETY: `cap > 0` when `len > 0`; first `len` elements are initialized.
        let slice = unsafe { core::slice::from_raw_parts_mut(self.data.as_ptr(), self.len) };
        // SAFETY: validity invariant.
        unsafe { Utf16Str::from_slice_unchecked_mut(slice) }
    }

    /// Return a raw const pointer to the string's `u16` elements.
    #[must_use]
    #[inline]
    pub const fn as_ptr(&self) -> *const u16 {
        self.data.as_ptr()
    }

    /// Return a raw mutable pointer to the string's `u16` elements.
    #[expect(clippy::needless_pass_by_ref_mut, reason = "API shape mirrors std::String::as_mut_ptr")]
    #[inline]
    pub const fn as_mut_ptr(&mut self) -> *mut u16 {
        self.data.as_ptr()
    }

    /// Construct an `Utf16String` containing `s`, copied into `arena`.
    #[must_use]
    pub fn from_utf16_str_in(s: &Utf16Str, arena: &'a Arena<A>) -> Self {
        let mut string = Self::new_in(arena);
        string.push_str(s);
        string
    }

    /// Construct an `Utf16String` by transcoding a `&str` into UTF-16,
    /// copied into `arena`.
    #[must_use]
    pub fn from_str_in(s: &str, arena: &'a Arena<A>) -> Self {
        let mut string = Self::new_in(arena);
        string.push_from_str(s);
        string
    }

    /// Remove the last character from the string and return it.
    pub fn pop(&mut self) -> Option<char> {
        let ch = self.as_utf16_str().chars().next_back()?;
        self.len -= ch.len_utf16();
        Some(ch)
    }

    /// Shorten the string to `new_len` `u16` elements.
    #[cfg_attr(test, mutants::skip)] // Equal-length truncate is a self-assignment.
    pub fn truncate(&mut self, new_len: usize) {
        if new_len >= self.len {
            return;
        }
        if new_len > 0 {
            let unit = self.as_slice()[new_len];
            assert!(
                !(0xDC00..=0xDFFF).contains(&unit),
                "truncate index {new_len} is not on a UTF-16 char boundary"
            );
        }
        self.len = new_len;
    }

    /// Try to release any unused capacity back to the chunk's bump cursor.
    #[cfg_attr(test, mutants::skip)] // Full-capacity shrink mutates to a zero-byte cursor reclaim.
    pub fn shrink_to_fit(&mut self) {
        if self.cap == 0 || self.len == self.cap {
            return;
        }
        let reclaim_units = self.cap - self.len;
        let reclaim_bytes = reclaim_units * core::mem::size_of::<u16>();
        // SAFETY: buffer_end = data + cap*2 bytes, within the same chunk payload.
        let buffer_end = unsafe { self.data.as_ptr().cast::<u8>().add(self.cap * core::mem::size_of::<u16>()) };
        // SAFETY: see above.
        if unsafe { self.arena.try_shrink_at_cursor(buffer_end, reclaim_bytes) } {
            self.cap = self.len;
        }
    }

    /// Append a single character.
    #[inline]
    pub fn push(&mut self, ch: char) {
        if self.try_push(ch).is_err() {
            panic_alloc();
        }
    }

    /// Fallible variant of [`Self::push`].
    #[inline]
    pub fn try_push(&mut self, ch: char) -> Result<(), AllocError> {
        let mut buf = [0_u16; 2];
        let encoded = ch.encode_utf16(&mut buf);
        self.try_push_slice(encoded)
    }

    /// Append a `Utf16Str`-like value.
    #[inline]
    pub fn push_str(&mut self, s: impl AsRef<Utf16Str>) {
        if self.try_push_str(s).is_err() {
            panic_alloc();
        }
    }

    /// Fallible variant of [`Self::push_str`].
    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "the hot path is bump-then-memcpy; the cold grow branch is `#[inline(never)]` so the inlinable body is small"
    )]
    pub fn try_push_str(&mut self, s: impl AsRef<Utf16Str>) -> Result<(), AllocError> {
        self.try_push_slice(s.as_ref().as_slice())
    }

    /// Append a `&str`-like value, transcoding it to UTF-16.
    #[inline]
    pub fn push_from_str(&mut self, s: impl AsRef<str>) {
        if self.try_push_from_str(s).is_err() {
            panic_alloc();
        }
    }

    /// Fallible variant of [`Self::push_from_str`].
    pub fn try_push_from_str(&mut self, s: impl AsRef<str>) -> Result<(), AllocError> {
        let s = s.as_ref();
        // Worst case, UTF-16 uses no more code units than the input uses UTF-8 bytes.
        let upper = s.len();
        let needed = self.len.checked_add(upper).ok_or(AllocError)?;
        if needed > self.cap {
            self.try_grow_to_at_least(needed)?;
        }
        for ch in s.chars() {
            let mut buf = [0_u16; 2];
            let encoded = ch.encode_utf16(&mut buf);
            // SAFETY: reserved enough above.
            unsafe {
                copy_nonoverlapping(encoded.as_ptr(), self.data.as_ptr().add(self.len), encoded.len());
            }
            self.len += encoded.len();
        }
        Ok(())
    }

    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "must inline so callers get a clean bump-then-memcpy hot path; cold grow path is `#[inline(never)]`"
    )]
    fn try_push_slice(&mut self, s: &[u16]) -> Result<(), AllocError> {
        let needed = self.len.checked_add(s.len()).ok_or(AllocError)?;
        if needed > self.cap {
            self.try_grow_to_at_least(needed)?;
        }
        // SAFETY: reserved enough above.
        unsafe { copy_nonoverlapping(s.as_ptr(), self.data.as_ptr().add(self.len), s.len()) };
        self.len = needed;
        Ok(())
    }

    /// Reserve capacity for at least `additional` more `u16` elements.
    #[inline]
    pub fn reserve(&mut self, additional: usize) {
        if self.try_reserve(additional).is_err() {
            panic_alloc();
        }
    }

    /// Fallible variant of [`Self::reserve`].
    #[inline]
    pub fn try_reserve(&mut self, additional: usize) -> Result<(), AllocError> {
        let needed = self.len.checked_add(additional).ok_or(AllocError)?;
        if needed > self.cap {
            self.try_grow_to_at_least(needed)?;
        }
        Ok(())
    }

    /// Insert a character at element index `idx`.
    pub fn insert(&mut self, idx: usize, ch: char) {
        let mut buf = [0_u16; 2];
        let encoded = ch.encode_utf16(&mut buf);
        self.insert_slice(idx, encoded);
    }

    /// Insert a `Utf16Str` at element index `idx`.
    pub fn insert_utf16_str(&mut self, idx: usize, s: &Utf16Str) {
        self.insert_slice(idx, s.as_slice());
    }

    #[cfg_attr(test, mutants::skip)] // Copy-count mutations are only distinguishable by UB tooling.
    fn insert_slice(&mut self, idx: usize, s: &[u16]) {
        assert!(idx <= self.len, "insert index out of bounds");
        if idx > 0 && idx < self.len {
            let unit = self.as_slice()[idx];
            assert!(
                !(0xDC00..=0xDFFF).contains(&unit),
                "insert index {idx} is not on a UTF-16 char boundary"
            );
        }
        let s_len = s.len();
        if s_len == 0 {
            return;
        }
        let needed = self.len.checked_add(s_len).expect("Utf16String overflow");
        if needed > self.cap {
            self.grow_to_at_least(needed);
        }
        // SAFETY: we have at least `needed` capacity now; shifting the
        // tail right by `s_len` keeps it inside the buffer.
        unsafe {
            let base = self.data.as_ptr();
            core::ptr::copy(base.add(idx), base.add(idx + s_len), self.len - idx);
            core::ptr::copy_nonoverlapping(s.as_ptr(), base.add(idx), s_len);
        }
        self.len = needed;
    }

    /// Remove the character at element index `idx` and return it.
    #[cfg_attr(test, mutants::skip)] // Copy-count mutations are only distinguishable by UB tooling.
    pub fn remove(&mut self, idx: usize) -> char {
        assert!(idx < self.len, "remove index out of bounds");
        let unit = self.as_slice()[idx];
        assert!(
            !(0xDC00..=0xDFFF).contains(&unit),
            "remove index {idx} is not on a UTF-16 char boundary"
        );
        let ch = self.as_utf16_str()[idx..].chars().next().expect("idx in bounds");
        let n = ch.len_utf16();
        // SAFETY: `idx + n <= len`; `ptr::copy` handles overlap.
        unsafe {
            let base = self.data.as_ptr();
            core::ptr::copy(base.add(idx + n), base.add(idx), self.len - idx - n);
        }
        self.len -= n;
        ch
    }

    /// Retain only the characters for which `f` returns `true`.
    ///
    /// # Panic safety
    ///
    /// If `f` panics, `self` is left **unchanged** (the original
    /// contents are preserved). This differs from
    /// [`crate::strings::String::retain`] which commits the
    /// already-processed prefix on panic. The difference is internal
    /// implementation detail: this variant uses a side buffer and
    /// only commits if the full pass completes without panicking,
    /// whereas the UTF-8 variant edits in place.
    ///
    /// # Allocator
    ///
    /// Allocates the side buffer from the **global** allocator, not
    /// the arena. Callers that require zero arena-foreign allocations
    /// in their hot path should avoid this method.
    pub fn retain<F: FnMut(char) -> bool>(&mut self, mut f: F) {
        let mut buf: alloc::vec::Vec<u16> = alloc::vec::Vec::with_capacity(self.len);
        for ch in self.as_utf16_str().chars() {
            if f(ch) {
                let mut tmp = [0_u16; 2];
                buf.extend_from_slice(ch.encode_utf16(&mut tmp));
            }
        }
        self.len = 0;
        if !buf.is_empty() {
            self.try_push_slice(&buf).expect("we already have capacity");
        }
    }

    /// Replace the elements in `range` with the contents of `replace_with`.
    #[cfg_attr(test, mutants::skip)] // Copy-count mutations are only distinguishable by UB tooling.
    pub fn replace_range<R: core::ops::RangeBounds<usize>>(&mut self, range: R, replace_with: &Utf16Str) {
        let start = match range.start_bound() {
            core::ops::Bound::Included(&n) => n,
            core::ops::Bound::Excluded(&n) => n
                .checked_add(1)
                .expect("excluded replace_range start bound must not overflow usize"),
            core::ops::Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            core::ops::Bound::Included(&n) => n.checked_add(1).expect("included replace_range end bound must not overflow usize"),
            core::ops::Bound::Excluded(&n) => n,
            core::ops::Bound::Unbounded => self.len,
        };
        assert!(
            start <= end && end <= self.len,
            "Utf16String::replace_range: bounds out of range (start={}, end={}, len={})",
            start,
            end,
            self.len
        );
        if start > 0 && start < self.len {
            let unit = self.as_slice()[start];
            assert!(
                !(0xDC00..=0xDFFF).contains(&unit),
                "Utf16String::replace_range start index {start} is not on a UTF-16 char boundary"
            );
        }
        if end > 0 && end < self.len {
            let unit = self.as_slice()[end];
            assert!(
                !(0xDC00..=0xDFFF).contains(&unit),
                "Utf16String::replace_range end index {end} is not on a UTF-16 char boundary"
            );
        }
        let removed = end - start;
        let inserted = replace_with.len();
        let new_len = self
            .len
            .checked_sub(removed)
            .expect("removed <= len")
            .checked_add(inserted)
            .expect("Utf16String overflow");
        if new_len > self.cap {
            self.grow_to_at_least(new_len);
        }
        // SAFETY: we have enough capacity; range and indices are valid.
        unsafe {
            let base = self.data.as_ptr();
            core::ptr::copy(base.add(end), base.add(start + inserted), self.len - end);
            core::ptr::copy_nonoverlapping(replace_with.as_slice().as_ptr(), base.add(start), inserted);
        }
        self.len = new_len;
    }

    fn grow_to_at_least(&mut self, min_cap: usize) {
        if self.try_grow_to_at_least(min_cap).is_err() {
            panic_alloc();
        }
    }

    /// Truncates this string, removing all contents. Capacity preserved.
    pub const fn clear(&mut self) {
        self.len = 0;
    }

    /// Freeze into an immutable [`RcUtf16Str<A>`].
    #[must_use]
    pub fn into_arena_utf16_str(self) -> RcUtf16Str<A> {
        if self.cap == 0 {
            let arena = self.arena;
            core::mem::forget(self);
            return arena.alloc_utf16_str_rc(widestring::utf16str!(""));
        }
        // SAFETY: cap > 0 implies allocation present; len <= cap.
        unsafe { self.flush_len_to_prefix() };
        self.try_reclaim_tail();
        let data = self.data;
        core::mem::forget(self);
        // SAFETY: data points at valid UTF-16 data; the +1 refcount transfers from this builder.
        unsafe { RcUtf16Str::from_raw_data(data) }
    }

    /// Freeze into an owned, mutable
    /// [`BoxUtf16Str<A>`](crate::strings::BoxUtf16Str).
    ///
    /// Same shape as [`Self::into_arena_utf16_str`] but produces an
    /// owned [`BoxUtf16Str`](crate::strings::BoxUtf16Str) whose `Drop`
    /// releases the chunk hold immediately.
    #[must_use]
    pub fn into_arena_box_utf16_str(self) -> crate::strings::BoxUtf16Str<A> {
        if self.cap == 0 {
            let arena = self.arena;
            core::mem::forget(self);
            return arena.alloc_utf16_str_box(widestring::utf16str!(""));
        }
        // SAFETY: cap > 0 implies allocation present; len <= cap.
        unsafe { self.flush_len_to_prefix() };
        self.try_reclaim_tail();
        let data = self.data;
        core::mem::forget(self);
        // SAFETY: data points at valid UTF-16 data; the +1 refcount transfers from this builder.
        unsafe { crate::strings::BoxUtf16Str::from_raw_data(data) }
    }

    /// Best-effort reclaim of unused tail bytes.
    #[inline]
    fn try_reclaim_tail(&self) {
        if self.len >= self.cap {
            return;
        }
        let prefix_size = core::mem::size_of::<usize>();
        let used = prefix_size + self.len * core::mem::size_of::<u16>();
        let total = prefix_size + self.cap * core::mem::size_of::<u16>();
        let reclaim = total - used;
        // SAFETY: prefix_ptr is the start of our allocation; buffer end is start + total.
        let prefix_ptr = unsafe { self.data.as_ptr().cast::<u8>().sub(prefix_size) };
        let buffer_end = unsafe { prefix_ptr.add(total) };
        // SAFETY: buffer_end == buffer_start + total; reclaim is the unused tail.
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
        // SAFETY: caller guarantees an allocation; data is just past the prefix slot.
        let prefix_ptr = unsafe { self.data.as_ptr().cast::<u8>().sub(prefix_size).cast::<usize>() };
        // SAFETY: this string exclusively owns the prefix slot and still owns
        // the allocation's chunk ref. The slot may already hold a stale
        // `usize` from an earlier flush, but `usize: Copy + !Drop`, so
        // overwriting it without running drop is always sound and still
        // satisfies `UninitSlot::from_raw`'s contract.
        let slot = unsafe { crate::internal::slot::UninitSlot::<usize>::from_raw(prefix_ptr) };
        slot.write_unaligned(self.len);
    }

    #[cold]
    #[inline(never)]
    fn try_allocate_initial(&mut self, cap: usize) -> Result<(), AllocError> {
        let prefix_size = core::mem::size_of::<usize>();
        let payload_bytes = cap.checked_mul(core::mem::size_of::<u16>()).ok_or(AllocError)?;
        let total = prefix_size.checked_add(payload_bytes).ok_or(AllocError)?;
        // Align for the `u16` payload; the prefix is accessed unaligned.
        let layout = Layout::from_size_align(total, core::mem::align_of::<u16>()).map_err(|_e| AllocError)?;
        let ptr = self.arena.allocate_layout(layout)?;
        // SAFETY: `ptr + prefix_size` lies within (or one past) the alloc.
        self.data = unsafe { NonNull::new_unchecked(ptr.as_ptr().add(prefix_size).cast::<u16>()) };
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
        let old_payload = self.cap.checked_mul(core::mem::size_of::<u16>()).ok_or(AllocError)?;
        let new_payload = new_cap.checked_mul(core::mem::size_of::<u16>()).ok_or(AllocError)?;
        let old_total = prefix_size.checked_add(old_payload).ok_or(AllocError)?;
        let new_total = prefix_size.checked_add(new_payload).ok_or(AllocError)?;
        // SAFETY: `cap > 0` so data is just past the prefix.
        let prefix_ptr = unsafe { NonNull::new_unchecked(self.data.as_ptr().cast::<u8>().sub(prefix_size)) };
        // SAFETY: prefix_ptr / old_total describe the existing allocation.
        let new_prefix = unsafe {
            self.arena
                .grow_for_string(prefix_ptr, old_total, new_total, core::mem::align_of::<u16>())?
        };
        let relocated = new_prefix != prefix_ptr;
        // Publish the new buffer before releasing the old chunk ref so an
        // unwind during release cannot make `Drop` see freed memory.
        // SAFETY: new_prefix is non-null, alloc covers `new_total` bytes.
        self.data = unsafe { NonNull::new_unchecked(new_prefix.as_ptr().add(prefix_size).cast::<u16>()) };
        self.cap = new_cap;
        if relocated {
            // SAFETY: prefix_ptr is in a local chunk and the old
            // allocation's +1 belongs to this string.
            let in_chunk = unsafe { crate::internal::in_chunk::InLocalChunk::<_, A>::new(prefix_ptr) };
            let chunk = in_chunk.chunk_ptr();
            // SAFETY: relocation released the old chunk's hold.
            unsafe { crate::internal::local_chunk::LocalChunk::dec_ref(chunk) };
        }
        Ok(())
    }
}

crate::arena_str_macros::impl_arena_string_forwarding_traits!(Utf16String, 'a, ::widestring::Utf16Str, as_utf16_str, as_mut_utf16_str);

impl<A: Allocator + Clone> Clone for Utf16String<'_, A> {
    fn clone(&self) -> Self {
        Self::from_utf16_str_in(self.as_utf16_str(), self.arena)
    }
}

impl<A: Allocator + Clone> Extend<char> for Utf16String<'_, A> {
    fn extend<I: IntoIterator<Item = char>>(&mut self, iter: I) {
        for ch in iter {
            self.push(ch);
        }
    }
}

impl<'a, A: Allocator + Clone> Extend<&'a Utf16Str> for Utf16String<'_, A> {
    fn extend<I: IntoIterator<Item = &'a Utf16Str>>(&mut self, iter: I) {
        for s in iter {
            self.push_str(s);
        }
    }
}

impl<'a, A: Allocator + Clone> Extend<&'a str> for Utf16String<'_, A> {
    fn extend<I: IntoIterator<Item = &'a str>>(&mut self, iter: I) {
        for s in iter {
            self.push_from_str(s);
        }
    }
}

#[cfg(feature = "serde")]
#[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
impl<A: Allocator + Clone> serde::ser::Serialize for Utf16String<'_, A> {
    fn serialize<S: serde::ser::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let owned = self.as_utf16_str().to_string();
        serializer.serialize_str(&owned)
    }
}

impl<A: Allocator + Clone> ::core::fmt::Write for Utf16String<'_, A> {
    fn write_str(&mut self, s: &str) -> ::core::fmt::Result {
        self.push_from_str(s);
        Ok(())
    }
}

impl<'a, A: Allocator + Clone> crate::vec::FromIteratorIn<char> for Utf16String<'a, A> {
    type Allocator = &'a Arena<A>;

    fn from_iter_in<I: IntoIterator<Item = char>>(iter: I, allocator: &'a Arena<A>) -> Self {
        let mut s = Self::new_in(allocator);
        s.extend(iter);
        s
    }
}

impl<'a, 'b, A: Allocator + Clone> crate::vec::FromIteratorIn<&'b Utf16Str> for Utf16String<'a, A> {
    type Allocator = &'a Arena<A>;

    fn from_iter_in<I: IntoIterator<Item = &'b Utf16Str>>(iter: I, allocator: &'a Arena<A>) -> Self {
        let mut s = Self::new_in(allocator);
        s.extend(iter);
        s
    }
}

impl<'a, 'b, A: Allocator + Clone> crate::vec::FromIteratorIn<&'b str> for Utf16String<'a, A> {
    type Allocator = &'a Arena<A>;

    fn from_iter_in<I: IntoIterator<Item = &'b str>>(iter: I, allocator: &'a Arena<A>) -> Self {
        let mut s = Self::new_in(allocator);
        s.extend(iter);
        s
    }
}
