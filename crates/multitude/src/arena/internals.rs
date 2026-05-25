// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Crate-internal helpers shared across the arena allocation modules.
//!
//! This includes sizing/predicate helpers, slice/drop support,
//! panic/expect wrappers, and the internal `Arena` trait impls.

use core::alloc::Layout;
use core::fmt;
use core::ptr::NonNull;

use allocator_api2::alloc::{AllocError, Allocator, Global};

use super::Arena;
use super::chunks::ChunkKind;
use crate::internal::drop_list::{DropEntry as InnerDropEntry, drop_shim_slice};
use crate::internal::local_chunk::LocalChunk;
use crate::internal::shared_chunk::SharedChunk;
use crate::internal::sync::Ordering;

pub(super) const fn compute_worst_case_size(layout: Layout, has_drop: bool) -> usize {
    let entry = if has_drop { core::mem::size_of::<InnerDropEntry>() } else { 0 };
    layout.size().saturating_add(layout.align()).saturating_add(entry)
}

/// Alias for `compute_worst_case_size(layout, entry_size != 0)`.
/// Kept separate so the `entry_size != 0` mutation can be skipped:
/// for `T: !Drop` it only over-requests, and for `T: Drop` it can turn
/// a tight-class allocation into a refill loop.
#[inline]
#[cfg_attr(test, mutants::skip)]
pub(super) const fn worst_case_refill_for(layout: Layout, entry_size: usize) -> usize {
    compute_worst_case_size(layout, entry_size != 0)
}

/// Truncate `value` to `u16` under a caller-asserted bound.
///
/// In debug builds the bound is checked with `debug_assert!`; in release
/// the optimizer is told via `assert_unchecked` so the conversion has no
/// panic surface. Centralizes the three-call `debug_assert! + assert_unchecked +
/// unwrap_unchecked` idiom used at every drop-entry install site.
///
/// # Safety
///
/// `value <= u16::MAX`. Violating this is UB in release builds.
#[inline(always)]
pub(super) unsafe fn u16_truncate_unchecked(value: usize) -> u16 {
    let res = u16::try_from(value);
    debug_assert!(res.is_ok(), "u16_truncate_unchecked: value {value} exceeds u16::MAX");
    // SAFETY: caller-asserted bound; debug builds panic above instead.
    unsafe { core::hint::assert_unchecked(res.is_ok()) };
    // SAFETY: caller-asserted bound; debug builds panic above instead.
    unsafe { res.unwrap_unchecked() }
}

/// In-payload u16 offset of `value_ptr` within `chunk`.
///
/// Folds the three-step `data_ptr + subtract + u16_truncate_unchecked`
/// dance used at every drop-entry install site into one helper, so the
/// chunk-liveness and offset-bound proofs live in a single place.
///
/// # Safety
///
/// - `chunk` must be live (refcount-positive) for the duration of the call.
/// - `value_ptr` must lie within `chunk`'s payload AND within the
///   max-bump-extent, so the offset fits in `u16`.
#[inline(always)]
pub(super) unsafe fn value_offset_in_chunk<C: ChunkKind + ?Sized>(chunk: NonNull<C>, value_ptr: NonNull<u8>) -> u16 {
    // SAFETY: caller asserts `chunk` is live.
    let payload_base_addr = unsafe { C::data_ptr_of(chunk) }.as_ptr() as usize;
    let raw_value_offset = (value_ptr.as_ptr() as usize) - payload_base_addr;
    // SAFETY: caller asserts the offset fits in `u16`.
    unsafe { u16_truncate_unchecked(raw_value_offset) }
}

/// `bumped > MAX_CHUNK_BYTES` boundary predicate.
/// Kept separate so the boundary mutation can be skipped: at
/// `bumped == MAX_CHUNK_BYTES`, both branches reach the same oversized
/// helper.
#[inline]
#[cfg_attr(test, mutants::skip)]
pub(super) const fn bumped_exceeds_chunk(bumped: usize) -> bool {
    bumped > crate::internal::constants::MAX_CHUNK_BYTES
}

/// `layout.size() > max_normal_alloc` boundary predicate.
/// Kept separate so the boundary mutation can be skipped: at the
/// boundary both routes end up in equivalent oversized helpers.
#[inline]
#[cfg_attr(test, mutants::skip)]
pub(super) const fn size_exceeds_normal_alloc(size: usize, max_normal_alloc: usize) -> bool {
    size > max_normal_alloc
}

/// `cur != target` chunk-eviction check for the closure path.
/// Kept separate so the `!=` mutation can be skipped: even if it forces
/// the recovery path, both branches converge on the same observable state.
#[inline]
#[cfg_attr(test, mutants::skip)]
pub(super) const fn current_chunk_evicted(cur_addr: usize, target_addr: usize) -> bool {
    cur_addr != target_addr
}

/// `entry_size > 0` predicate for slice-init fast paths.
/// Kept separate so the `>` mutation can be skipped: for non-drop
/// slices it would only install a phantom zero-length noop entry.
#[inline]
#[cfg_attr(test, mutants::skip)]
pub(super) const fn has_drop_entry(entry_size: usize) -> bool {
    entry_size > 0
}

/// Refill-budget arithmetic for value-path slow routing.
/// Kept separate so arithmetic mutations can be skipped: for common
/// alignments the difference vanishes or is swallowed by chunk-class
/// rounding.
#[inline]
#[cfg_attr(test, mutants::skip)]
pub(super) const fn slow_refill_needed(layout: Layout, entry_size: usize) -> usize {
    layout.size() + layout.align().saturating_sub(core::mem::align_of::<usize>()) + entry_size
}

#[inline(never)]
pub(super) fn drop_fn_for_slice<T>() -> Option<unsafe fn(*mut u8, usize)> {
    let f: unsafe fn(*mut u8, usize) = drop_shim_slice::<T>;
    needs_drop_indirect::<T>().then_some(f)
}

#[inline(never)]
#[cfg_attr(test, mutants::skip)] // Non-Drop shims are no-ops; only reserved entry size changes.
pub(super) fn needs_drop_indirect<T>() -> bool {
    core::mem::needs_drop::<T>()
}

pub(super) struct SliceInitGuard<T> {
    pub(super) ptr: *mut T,
    pub(super) len: usize,
}

impl<T> Drop for SliceInitGuard<T> {
    fn drop(&mut self) {
        for i in 0..self.len {
            // SAFETY: the guard tracks exactly the initialized prefix.
            unsafe { core::ptr::drop_in_place(self.ptr.add(i)) };
        }
    }
}

/// Threshold (in bytes) above which `T`'s stack alignment is offloaded to
/// [`write_through_ptr_outlined`] to keep the caller's frame slim. The
/// default `x86_64` stack is 16-byte-aligned; types whose alignment exceeds
/// that force the caller to dynamically realign its frame, which is
/// expensive and pessimizes the common case.
pub(super) const WRITE_THROUGH_INLINE_ALIGN: usize = 16;

/// Initialize the slot at `ptr` with `f()`.
///
/// On the hot path (`align_of::<T>() <= WRITE_THROUGH_INLINE_ALIGN`) this
/// is a forced inline so LLVM sees the closure call site directly and can
/// reason about whether the closure touches arena state — the difference
/// between a forced post-closure reload of `current_local` and an elided
/// one. Exotic high-alignment `T` is dispatched to
/// [`write_through_ptr_outlined`] so the caller's frame is unaffected.
///
/// # Safety
/// Caller guarantees `ptr` is valid for writing a `T`.
#[inline(always)]
#[expect(
    clippy::inline_always,
    reason = "must inline so LLVM sees the closure call site directly and can prove whether the closure touches `current_local`; without that proof it forces a reload of every Cell field after the call, undoing the entire fast-path win"
)]
pub(super) unsafe fn write_through_ptr<T, F: FnOnce() -> T>(ptr: *mut T, f: F) {
    if const { core::mem::align_of::<T>() <= WRITE_THROUGH_INLINE_ALIGN } {
        // SAFETY: caller guarantees `ptr` is valid for writing a `T`.
        unsafe { core::ptr::write(ptr, f()) };
    } else {
        // SAFETY: forwarded to the outlined helper; same precondition.
        unsafe { write_through_ptr_outlined::<T, F>(ptr, f) };
    }
}

/// Outlined fallback used when `T`'s alignment exceeds the inline
/// threshold; isolates the high-alignment frame requirements from the
/// caller. See [`write_through_ptr`].
#[inline(never)]
#[cold]
pub(super) unsafe fn write_through_ptr_outlined<T, F: FnOnce() -> T>(ptr: *mut T, f: F) {
    // SAFETY: caller guarantees `ptr` is valid for writing a `T`.
    unsafe { core::ptr::write(ptr, f()) };
}

#[inline(never)]
#[cold]
#[expect(clippy::panic, reason = "panicking allocation entry points panic on alloc failure by design")]
pub fn panic_alloc() -> ! {
    panic!("multitude: allocator returned AllocError");
}

/// Unwrap a `Result<T, AllocError>` or invoke [`panic_alloc`].
///
/// All public infallible-allocation entry points (`alloc_*`,
/// `alloc_slice_*`, etc.) forward to their `try_alloc_*` counterparts
/// through this helper, so the call shape is shared across roughly 46 sites
/// without per-site duplication of the `match { Ok / Err / panic_alloc }`
/// body. Compiles to the same instructions as the inline `match`
/// because `panic_alloc` is `#[cold] #[inline(never)]` and this
/// function is `#[inline]`.
#[inline]
pub fn expect_alloc<T>(r: Result<T, AllocError>) -> T {
    match r {
        Ok(v) => v,
        Err(_) => panic_alloc(),
    }
}

/// Overflow guard for slice-layout arithmetic.
///
/// Returns `Err(AllocError)` when `total` bytes plus worst-case alignment
/// padding would exceed `isize::MAX`. On 64-bit platforms this is
/// unreachable for slices obtained by reference (whose byte length is
/// bounded by the address space), but is necessary for callers that accept
/// a raw `len: usize`.
#[expect(clippy::inline_always, reason = "zero-cost wrapper must inline at call site")]
#[inline(always)]
#[cfg_attr(test, mutants::skip)] // Boundary requires an allocation beyond practical allocator limits.
pub(crate) const fn check_isize_overflow(total: usize, align: usize) -> Result<(), AllocError> {
    let padding = align.saturating_sub(1);
    if total > (isize::MAX as usize).saturating_sub(padding) {
        Err(AllocError)
    } else {
        Ok(())
    }
}

impl Default for Arena<Global> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A: Allocator + Clone> Drop for Arena<A> {
    fn drop(&mut self) {
        self.reset();
        // After `reset` no `+1`s are held by the arena; dropping
        // `provider` drops the chunk-provider; surviving chunks held
        // by smart pointers self-free through their own allocator
        // clones when the last smart pointer releases.
    }
}

impl<A: Allocator + Clone> fmt::Debug for Arena<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut dbg = f.debug_struct("Arena");
        #[cfg(feature = "stats")]
        dbg.field("stats", &self.stats());
        dbg.finish_non_exhaustive()
    }
}

/// Selects the side-effect of `try_alloc_inner_with` once it has
/// successfully reserved space for a `T` and built it.
#[derive(Clone, Copy)]
pub(super) enum AllocFlavor {
    /// Bump-reference allocation — no extra +1; mark
    /// `current_local.is_pinned`.
    SimpleRef,
    /// `Rc` allocation — bump the chunk's refcount by an extra +1
    /// for the smart pointer to own.
    Rc,
    /// `Box` allocation — bump the chunk's refcount by an extra +1
    /// but do NOT install a trailing drop entry (Box runs `drop_in_place`
    /// directly and releases its +1 at that point).
    Box,
}

/// RAII guard that releases the protective hold on `chunk` if the
/// user's init closure panics. The success path forgets the guard.
///
/// For `SimpleRef`, the protective hold is the `is_pinned` mark on
/// `current_local` — we leave that mark in place on panic (clearing
/// it could un-pin a chunk pinned by *prior* `SimpleRef` allocations).
/// For `Rc`/`Box`, the protective hold is a pre-closure bump of
/// `current_local.smart_pointers_issued`. On panic we undo it: if the chunk is
/// still current, decrement the counter; if it was reconciled out
/// during the closure, the +1 is now in the chunk's actual refcount,
/// so we `dec_ref` instead.
pub(super) struct ProtectiveHold<'a, A: Allocator + Clone> {
    pub(super) arena: &'a Arena<A>,
    pub(super) chunk: NonNull<LocalChunk<A>>,
    pub(super) flavor: AllocFlavor,
}

impl<A: Allocator + Clone> Drop for ProtectiveHold<'_, A> {
    #[cold]
    fn drop(&mut self) {
        match self.flavor {
            AllocFlavor::SimpleRef => {
                // Do not clear `is_pinned`; prior SimpleRef allocations may rely on it.
            }
            AllocFlavor::Rc | AllocFlavor::Box => {
                if self.arena.current_local.chunk.get() == Some(self.chunk) {
                    let cur = self.arena.current_local.smart_pointers_issued.get();
                    debug_assert!(
                        cur > 0,
                        "ProtectiveHold::drop fired with smart_pointers_issued == 0; the pre-closure bump was either skipped or already undone"
                    );
                    self.arena.current_local.smart_pointers_issued.set(cur - 1);
                } else {
                    // SAFETY: swap-out reconcile transferred this +1 to the chunk.
                    unsafe { self.arena.release_local_chunk(self.chunk) };
                }
            }
        }
    }
}

/// Undoes a pre-closure `arcs_issued` bump if initialization panics.
pub(super) struct SharedArcsIssuedHold<'a, A: Allocator + Clone> {
    pub(super) arena: &'a Arena<A>,
    pub(super) chunk: NonNull<SharedChunk<A>>,
}

impl<A: Allocator + Clone> Drop for SharedArcsIssuedHold<'_, A> {
    #[cold]
    fn drop(&mut self) {
        if self.arena.current_shared.chunk.get() == Some(self.chunk) {
            let cur = self.arena.current_shared.smart_pointers_issued.get();
            debug_assert!(
                cur > 0,
                "SharedArcsIssuedHold::drop fired with smart_pointers_issued == 0; the pre-closure bump was either skipped or already undone"
            );
            self.arena.current_shared.smart_pointers_issued.set(cur - 1);
        } else {
            // SAFETY: swap-out reconcile transferred this +1 to the chunk.
            unsafe { SharedChunk::dec_ref(self.chunk) };
        }
    }
}

/// Bump a local chunk's `drop_count` by 1.
///
/// For chunks that become `current_local`, this is redundant with the
/// `mirror_dc` value `reconcile_swap_out` derives from `drop_back` — the
/// caller has already advanced `drop_back` by `entry_size` so the
/// mirrored count would be `≥ explicit_count`. For **oversized one-shot
/// chunks**, however, the chunk never becomes `current_local` and
/// `reconcile_swap_out` is not called; `replay_drops` reads
/// `chunk.drop_count` directly, so this explicit bump is load-bearing.
/// Do not remove it.
///
/// # Safety
///
/// `chunk` must be live (refcount-positive).
#[inline(always)]
#[cfg_attr(test, mutants::skip)]
pub(super) unsafe fn bump_local_drop_count<A: Allocator + Clone>(chunk: NonNull<LocalChunk<A>>) {
    // SAFETY: caller guarantees chunk is live (refcount-positive).
    let dc = &unsafe { chunk.as_ref() }.drop_count;
    dc.set(dc.get() + 1);
}

/// Bump a shared chunk's `drop_count` by 1 with Release ordering.
///
/// Same load-bearing-vs-redundant story as [`bump_local_drop_count`]
/// (redundant for `current_shared` chunks, load-bearing for oversized
/// one-shot shared chunks). On shared chunks the bump uses `fetch_add`
/// with `Ordering::Release` to publish the noop entry write to any
/// foreign thread that later loads with Acquire.
///
/// # Safety
///
/// `chunk` must be live (refcount-positive).
#[inline(always)]
#[cfg_attr(test, mutants::skip)]
pub(super) unsafe fn bump_shared_drop_count<A: Allocator + Clone>(chunk: NonNull<SharedChunk<A>>) {
    // SAFETY: caller guarantees chunk is live (refcount-positive).
    unsafe { chunk.as_ref() }.drop_count.fetch_add(1, Ordering::Release);
}

/// Bump `value` up to the next multiple of `align`. `align` must be a
/// power of two.
///
/// Returns `usize::MAX & !(align - 1)` on overflow (i.e. when
/// `value + (align - 1)` would wrap). Every hot-path call site
/// follows this with a saturating-bound check, which converts the
/// saturated value into a slow-path `AllocError` rather than silently
/// producing a bogus pointer below the real end.
///
/// Branchless on x86-64: `add` + `cmovc` + `and`, no conditional
/// jumps. Lowering relies on `saturating_add` being implemented as
/// add+cmov and the compiler folding the constant `align - 1` /
/// `!(align - 1)` masks for known `T`.
#[inline]
#[cfg(feature = "dst")]
pub(super) fn align_up(value: usize, align: usize) -> usize {
    debug_assert!(align.is_power_of_two());
    value.saturating_add(align - 1) & !(align - 1)
}

/// Result of [`try_bump_fit`].
///
/// Returned by value rather than wrapped in `Option<...>` to avoid a
/// dead niche-check at the call site. With `Option<(NonNull, NonNull, NonNull)>`,
/// LLVM folds the `None` discriminant into one of the
/// `NonNull` niches; an `assert_unchecked(!ptr.is_null())` on the
/// chosen carrier is not reliably propagated through `byte_add`'s
/// provenance semantics, so the call site keeps a null-check and
/// conditional branch pair on the hot path. Carrying the discriminant in a
/// separate `bool` field eliminates the niche game entirely; with
/// `try_bump_fit` `inline(always)`, SROA breaks this struct into
/// individual registers and `fits` becomes a flag-register condition.
#[derive(Clone, Copy)]
pub(super) struct BumpFit {
    /// `true` iff the requested allocation fits.
    pub(super) fits: bool,
    /// Aligned start of the new allocation. Meaningful only when
    /// `fits == true`; `NonNull::dangling()` otherwise.
    pub(super) aligned_ptr: NonNull<u8>,
    /// One byte past the end of the new allocation (the new bump
    /// cursor). Meaningful only when `fits == true`;
    /// `NonNull::dangling()` otherwise.
    pub(super) end_ptr: NonNull<u8>,
    /// New back-stack limit pointer (`drop_back - entry_size`).
    /// Meaningful only when `fits == true`; `NonNull::dangling()`
    /// otherwise.
    pub(super) new_drop_back_ptr: NonNull<u8>,
}

/// Single-branch bump-fit check used by every alloc fast path.
///
/// Returns a [`BumpFit`] with `fits == true` and three populated
/// pointer fields when the requested allocation fits in
/// `[data_addr, drop_back_addr)` after alignment and reserving
/// `entry_size` bytes at the back-stack; otherwise returns
/// `BumpFit { fits: false, .. }` with `dangling()` pointer fields
/// (route to the slow path).
///
/// Lowers to a single conditional branch on x86-64: two
/// `saturating_sub`s (`sub`+`cmov`), one `align_up` (`add`+`cmovc`+`and`),
/// one `cmp`+`ja`, and two `add`s. Replaces the previous three-branch
/// sequence (`align_up` overflow, `aligned + bumped` overflow,
/// `end > drop_back` bound).
#[inline(always)]
#[cfg_attr(test, mutants::skip)] // Exact-fit misses route to the slow path, which still satisfies the allocation.
pub(super) fn try_bump_fit(data_ptr: NonNull<u8>, drop_back_ptr: NonNull<u8>, align: usize, bumped: usize, entry_size: usize) -> BumpFit {
    debug_assert!(align.is_power_of_two());
    debug_assert!(align >= 1);
    debug_assert!(bumped >= 1, "callers must pass `bumped >= 1` (use `layout.size().max(1)`)");
    let data_addr = data_ptr.as_ptr() as usize;
    let drop_back_addr = drop_back_ptr.as_ptr() as usize;
    // SAFETY: real chunk payloads come from a system allocation
    // whose top sits well below `usize::MAX` (≤ 2^48 on every real
    // 64-bit platform); stub state uses `data_addr == 1`. The bound
    // `data_addr <= isize::MAX as usize` is satisfied by every real
    // user-space allocation on x86-64 / aarch64 (canonical
    // user-space ≤ 47 bits) and by the dangling stub (`data_addr ==
    // 1`). Asserting it lets `aligned + bumped + entry_size` lower
    // to plain `add` without saturating-arithmetic guards: every
    // caller bounds `bumped <= isize::MAX - padding` (via
    // `check_isize_overflow` on slice paths or
    // `assert_unchecked(bumped <= MAX_CHUNK_BYTES)` on value paths),
    // and `entry_size` is always `0` or `size_of::<InnerDropEntry>()`
    // (≤ 32 bytes), so `bumped + entry_size <= isize::MAX` and
    // `aligned + (bumped + entry_size) <= 2 * isize::MAX < usize::MAX`.
    unsafe {
        core::hint::assert_unchecked(data_addr > 0);
        core::hint::assert_unchecked(isize::try_from(data_addr).is_ok());
        core::hint::assert_unchecked(isize::try_from(bumped).is_ok());
        core::hint::assert_unchecked(isize::try_from(entry_size).is_ok());
    }
    let aligned = (data_addr + (align - 1)) & !(align - 1);
    // Plain fit check: `aligned + bumped + entry_size <= drop_back_addr`.
    // The sum cannot overflow under the bounds asserted above.
    let end = aligned + bumped + entry_size;
    if end > drop_back_addr {
        // No-fit shape: `dangling()` sentinels keep the `BumpFit`
        // valid (all `NonNull`s non-null) without any out-of-provenance
        // pointer arithmetic. Caller must not observe these when
        // `fits == false`.
        return BumpFit {
            fits: false,
            aligned_ptr: NonNull::dangling(),
            end_ptr: NonNull::dangling(),
            new_drop_back_ptr: NonNull::dangling(),
        };
    }
    // Provenance-preserving pointer construction.
    let aligned_offset = aligned - data_addr;
    let bumped_offset = aligned_offset + bumped;
    // SAFETY: the chunk allocation invariant: `data_ptr` has
    // provenance for `[data_ptr, data_ptr + bump_extent)` and
    // `drop_back_ptr` has provenance up through its current limit.
    // `aligned`, `aligned + bumped`, and `drop_back - entry_size`
    // all lie inside the corresponding range (gated by the fit
    // check above). All three resulting addresses are non-zero
    // because `aligned >= align >= 1`, `aligned + bumped >= 1`, and
    // `drop_back_addr - entry_size >= aligned + bumped > 0`.
    unsafe {
        let aligned_raw = data_ptr.as_ptr().byte_add(aligned_offset);
        let end_raw = data_ptr.as_ptr().byte_add(bumped_offset);
        let new_drop_back_raw = drop_back_ptr.as_ptr().byte_sub(entry_size);
        BumpFit {
            fits: true,
            aligned_ptr: NonNull::new_unchecked(aligned_raw),
            end_ptr: NonNull::new_unchecked(end_raw),
            new_drop_back_ptr: NonNull::new_unchecked(new_drop_back_raw),
        }
    }
}

/// Returns the byte offset from `value` to the next `align`-aligned
/// address, or `None` if that address would not fit in `usize`.
#[inline]
pub(super) fn align_offset(value: usize, align: usize) -> Option<usize> {
    debug_assert!(align.is_power_of_two());
    let aligned = value.checked_add(align - 1)? & !(align - 1);
    Some(aligned - value)
}

/// Computes the aligned offset of a payload of `size` bytes inside a
/// chunk of `cap` bytes that reserves `reserved_tail` bytes at the
/// end (e.g. for drop-back entries), feeding optimizer hints that
/// align with the chunk provider's post-condition.
///
/// # Safety
///
/// Caller guarantees, via the chunk provider's post-condition, that
/// the request fits the chunk: `align_offset(data_addr, align)` does
/// not overflow, `aligned + size` does not overflow, and
/// `aligned + size <= cap - reserved_tail`.
#[inline]
pub(super) unsafe fn aligned_payload_offset(data_addr: usize, align: usize, size: usize, cap: usize, reserved_tail: usize) -> usize {
    // SAFETY: the chunk-provider post-condition cited by the caller
    // proves each of these unchecked operations succeeds.
    unsafe {
        let aligned = align_offset(data_addr, align).unwrap_unchecked();
        let end = aligned.checked_add(size).unwrap_unchecked();
        core::hint::assert_unchecked(end <= cap.saturating_sub(reserved_tail));
        aligned
    }
}

impl<A: Allocator + Clone> Arena<A> {
    /// Record the bytes allocated in stats.
    #[inline]
    #[cfg_attr(
        not(feature = "stats"),
        expect(
            clippy::missing_const_for_fn,
            reason = "non-const under stats feature; bump_stat! invokes Cell::set"
        )
    )]
    #[cfg_attr(
        not(feature = "stats"),
        expect(clippy::unused_self, reason = "no-op stub when `stats` feature is disabled")
    )]
    pub(crate) fn charge_alloc_stats(
        &self,
        #[cfg_attr(
            not(feature = "stats"),
            expect(unused_variables, reason = "no-op stub when `stats` feature is disabled")
        )]
        bytes: usize,
    ) {
        #[cfg(feature = "stats")]
        crate::arena_stats::StatsStorage::add(&self.provider.stats.total_bytes_allocated, bytes as u64);
    }

    /// Bump `relocations` by 1. Called whenever a growing collection
    /// has to be moved to a fresh, larger buffer. No-op when the
    /// `stats` feature is disabled.
    #[inline]
    #[cfg_attr(
        not(feature = "stats"),
        expect(
            clippy::missing_const_for_fn,
            reason = "non-const under stats feature; bump_stat! invokes Cell::set"
        )
    )]
    #[cfg_attr(
        not(feature = "stats"),
        expect(clippy::unused_self, reason = "no-op stub when `stats` feature is disabled")
    )]
    pub(crate) fn bump_relocation(&self) {
        #[cfg(feature = "stats")]
        crate::arena_stats::StatsStorage::add(&self.provider.stats.relocations, 1);
    }
}
