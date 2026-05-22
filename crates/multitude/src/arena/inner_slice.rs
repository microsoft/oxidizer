// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Crate-private slice allocation helpers shared by the arena APIs.
//!
//! This module holds the local/shared slice fast paths plus the cold
//! slow and oversized paths.

use core::alloc::Layout;
use core::mem::MaybeUninit;
use core::ptr::NonNull;

use allocator_api2::alloc::{AllocError, Allocator};

use super::{
    AllocFlavor, Arena, OversizedLocalGuard, OversizedSharedGuard, ProtectiveHold, SharedArcsIssuedHold, SliceInitGuard, align_offset,
    bump_local_drop_count, bump_shared_drop_count, compute_worst_case_size, expect_alloc, has_drop_entry, panic_alloc,
    size_exceeds_normal_alloc, slow_refill_needed, try_bump_fit, worst_case_refill_for,
};
use crate::internal::constants::MAX_SMART_PTR_ALIGN;
use crate::internal::drop_list::{DropEntry as InnerDropEntry, noop_drop_shim};
use crate::internal::local_chunk::LocalChunk;
use crate::internal::shared_chunk::SharedChunk;
use crate::internal::sync::Ordering;

impl<A: Allocator + Clone> Arena<A> {
    /// Cold one-shot oversized path for local-flavor slices.
    ///
    /// This mirrors [`Self::try_alloc_inner_oversized_with`] but keeps
    /// oversized chunks out of `current_local`, preserving the
    /// chunk-header mask invariant.
    #[cold]
    #[inline(never)]
    pub(super) fn try_alloc_slice_local_oversized_with<T, F>(
        &self,
        len: usize,
        flavor: AllocFlavor,
        drop_fn: Option<unsafe fn(*mut u8, usize)>,
        mut init: F,
    ) -> Result<NonNull<[T]>, AllocError>
    where
        F: FnMut(usize, &mut MaybeUninit<T>),
    {
        let layout = Layout::array::<T>(len).map_err(|_e| AllocError)?;
        // Caller is responsible for the per-flavor alignment cap
        // (`MAX_SMART_PTR_ALIGN` for drop-aware paths, `CHUNK_ALIGN`
        // for Copy paths). Both caps ensure the aligned offset and the
        // value start address still mask back to this chunk's header.
        debug_assert!(layout.align() < crate::internal::constants::CHUNK_ALIGN);
        // Layout::array<T>(len) already enforces size_aligned <= isize::MAX,
        // which subsumes `check_isize_overflow`.
        let entry_size = if drop_fn.is_some() && len != 0 {
            core::mem::size_of::<InnerDropEntry>()
        } else {
            0
        };
        if entry_size != 0 && len > u16::MAX as usize {
            return Err(AllocError);
        }

        // `layout.size() <= isize::MAX` (from Layout::array), `layout.align() < CHUNK_ALIGN < isize::MAX`,
        // and `entry_size <= size_of::<InnerDropEntry>()`, so the sum cannot overflow `usize`.
        let needed = slow_refill_needed(layout, entry_size);
        let chunk = self.provider.acquire_local(needed)?;
        // SAFETY: refcount-positive — LARGE inflation keeps the chunk live.
        let chunk_ref = unsafe { chunk.as_ref() };
        // SAFETY: refcount-positive — chunk held at LARGE inflation.
        let data_ptr = unsafe { LocalChunk::<A>::data_ptr(chunk) };
        let cap = chunk_ref.capacity;
        let data_addr = data_ptr.as_ptr() as usize;
        // SAFETY: provider post-condition guarantees the chunk fits the request after
        // alignment and drop-entry slack, so neither computation below can fail.
        let aligned = unsafe { align_offset(data_addr, layout.align().max(1)).unwrap_unchecked() };
        // SAFETY: same post-condition; computation fits well below `usize::MAX`.
        let end = unsafe { aligned.checked_add(layout.size()).unwrap_unchecked() };
        // SAFETY: same post-condition.
        unsafe { core::hint::assert_unchecked(end <= cap.saturating_sub(entry_size)) };
        // SAFETY: payload-extent invariant — `aligned` is `T`-aligned and within `[0, cap)`.
        let value_ptr: *mut T = unsafe { data_ptr.as_ptr().add(aligned).cast::<T>() };
        let _ = end;

        let chunk_guard = OversizedLocalGuard { chunk };
        let mut init_guard = SliceInitGuard { ptr: value_ptr, len: 0 };
        // SAFETY: `value_ptr` is aligned, non-null, and covers `len`
        // freshly-reserved `MaybeUninit<T>` slots in the chunk payload.
        let slots: &mut [MaybeUninit<T>] = unsafe { core::slice::from_raw_parts_mut(value_ptr.cast::<MaybeUninit<T>>(), len) };
        for (i, slot) in slots.iter_mut().enumerate() {
            init(i, slot);
            init_guard.len += 1;
        }
        core::mem::forget(init_guard);

        if entry_size > 0 {
            let new_drop_back = cap - entry_size;
            #[expect(
                clippy::cast_ptr_alignment,
                reason = "chunk payloads are 64 KiB aligned (CHUNK_ALIGN), so any `InnerDropEntry` slot computed as `data + new_drop_back` is naturally aligned for `InnerDropEntry`"
            )]
            // SAFETY: payload-extent invariant — back-stack slot lies within payload.
            let entry_ptr = unsafe { data_ptr.as_ptr().add(new_drop_back).cast::<InnerDropEntry>() };
            let installed_drop_fn = if matches!(flavor, AllocFlavor::Box) {
                noop_drop_shim
            } else {
                // SAFETY: `entry_size > 0` implies `drop_fn.is_some()`.
                unsafe { drop_fn.unwrap_unchecked() }
            };
            let value_offset_u16 = u16::try_from(aligned)
                .expect("oversized chunk payload starts at offset 0; aligned < align < MAX_SMART_PTR_ALIGN ≤ u16::MAX");
            let len_u16 = u16::try_from(len).expect("guarded above: entry_size > 0 implies len ≤ u16::MAX");
            let entry = InnerDropEntry::new(installed_drop_fn, value_offset_u16, len_u16);
            // SAFETY: payload-extent invariant.
            unsafe { core::ptr::write(entry_ptr, entry) };
            chunk_ref.drop_count.set(1);
        }

        self.charge_alloc_stats(layout.size());
        core::mem::forget(chunk_guard);

        match flavor {
            AllocFlavor::Rc | AllocFlavor::Box => {
                // SAFETY: chunk held LARGE while we acted as its sole tenant.
                unsafe { LocalChunk::reconcile_swap_out(chunk, 1, false) };
            }
            AllocFlavor::SimpleRef => {
                let head = self.pinned_local.replace(None);
                chunk_ref.next.set(head);
                self.pinned_local.set(Some(chunk));
                // SAFETY: chunk held LARGE; rcs_issued = 0, pinned = true → leaves +1 for the pin.
                unsafe { LocalChunk::reconcile_swap_out(chunk, 0, true) };
            }
        }

        let fat = core::ptr::slice_from_raw_parts_mut(value_ptr, len);
        // SAFETY: `fat` is non-null and covers initialized elements.
        Ok(unsafe { NonNull::new_unchecked(fat) })
    }

    /// Cold one-shot oversized-allocation path for shared-flavor (Arc) slices.
    ///
    /// Mirror of [`Self::try_alloc_inner_arc_oversized_with`] for the
    /// slice case. Slice fast paths delegate here whenever
    /// `layout.size() > max_normal_alloc` to avoid routing the
    /// oversized shared chunk through `current_shared`.
    #[inline(never)]
    pub(super) fn try_alloc_slice_shared_oversized_with<T, F>(
        &self,
        len: usize,
        drop_fn: Option<unsafe fn(*mut u8, usize)>,
        mut init: F,
    ) -> Result<NonNull<[T]>, AllocError>
    where
        F: FnMut(usize, &mut MaybeUninit<T>),
    {
        let layout = Layout::array::<T>(len).map_err(|_e| AllocError)?;
        // Caller validates the per-flavor alignment cap; see
        // [`Self::try_alloc_slice_local_oversized_with`].
        debug_assert!(layout.align() < crate::internal::constants::CHUNK_ALIGN);
        // Layout::array enforces `size_aligned <= isize::MAX`.
        let entry_size = if drop_fn.is_some() && len != 0 {
            core::mem::size_of::<InnerDropEntry>()
        } else {
            0
        };
        if entry_size != 0 && len > u16::MAX as usize {
            return Err(AllocError);
        }

        // Bounded by Layout::array's `size <= isize::MAX` plus a small constant.
        let needed = slow_refill_needed(layout, entry_size);
        let chunk = self.provider.acquire_shared(needed)?;
        // SAFETY: refcount-positive — LARGE inflation keeps the chunk live.
        let chunk_ref = unsafe { chunk.as_ref() };
        // SAFETY: refcount-positive — chunk held at LARGE inflation.
        let data_ptr = unsafe { SharedChunk::<A>::data_ptr(chunk) };
        let cap = chunk_ref.capacity;
        let data_addr = data_ptr.as_ptr() as usize;
        // SAFETY: provider post-condition.
        let aligned = unsafe { align_offset(data_addr, layout.align().max(1)).unwrap_unchecked() };
        // SAFETY: same post-condition.
        let end = unsafe { aligned.checked_add(layout.size()).unwrap_unchecked() };
        // SAFETY: same post-condition.
        unsafe { core::hint::assert_unchecked(end <= cap.saturating_sub(entry_size)) };
        // SAFETY: payload-extent invariant.
        let value_ptr: *mut T = unsafe { data_ptr.as_ptr().add(aligned).cast::<T>() };
        let _ = end;

        let chunk_guard = OversizedSharedGuard { chunk };
        let mut init_guard = SliceInitGuard { ptr: value_ptr, len: 0 };
        // SAFETY: `value_ptr` covers `len` `MaybeUninit<T>` slots.
        let slots: &mut [MaybeUninit<T>] = unsafe { core::slice::from_raw_parts_mut(value_ptr.cast::<MaybeUninit<T>>(), len) };
        for (i, slot) in slots.iter_mut().enumerate() {
            init(i, slot);
            init_guard.len += 1;
        }
        core::mem::forget(init_guard);

        if entry_size > 0 {
            let new_drop_back = cap - entry_size;
            #[expect(
                clippy::cast_ptr_alignment,
                reason = "chunk payloads are 64 KiB aligned (CHUNK_ALIGN), so any `InnerDropEntry` slot computed as `data + new_drop_back` is naturally aligned for `InnerDropEntry`"
            )]
            // SAFETY: payload-extent invariant.
            let entry_ptr = unsafe { data_ptr.as_ptr().add(new_drop_back).cast::<InnerDropEntry>() };
            // SAFETY: `entry_size > 0` implies `drop_fn.is_some()`.
            let installed_drop_fn = unsafe { drop_fn.unwrap_unchecked() };
            let value_offset_u16 = u16::try_from(aligned)
                .expect("oversized chunk payload starts at offset 0; aligned < align < MAX_SMART_PTR_ALIGN ≤ u16::MAX");
            let len_u16 = u16::try_from(len).expect("guarded above: entry_size > 0 implies len ≤ u16::MAX");
            let entry = InnerDropEntry::new(installed_drop_fn, value_offset_u16, len_u16);
            // SAFETY: payload-extent invariant.
            unsafe { core::ptr::write(entry_ptr, entry) };
            // No other thread can yet observe this chunk: the
            // inflation has not been published via any cross-thread handoff.
            chunk_ref.drop_count.store(1, Ordering::Relaxed);
        }

        self.charge_alloc_stats(layout.size());
        core::mem::forget(chunk_guard);

        // SAFETY: chunk held LARGE while we acted as its sole tenant.
        unsafe { SharedChunk::reconcile_swap_out(chunk, 1) };

        let fat = core::ptr::slice_from_raw_parts_mut(value_ptr, len);
        // SAFETY: `fat` is non-null and covers initialized elements.
        Ok(unsafe { NonNull::new_unchecked(fat) })
    }

    /// Reserve space for a `T` in the current local chunk, refilling
    /// if necessary, then build the value via `f` and write it into
    /// the slot.
    ///
    /// Returns a [`NonNull<T>`] *with the chunk's refcount already
    /// incremented by one* (one extra +1 beyond the
    /// `current_local`'s) for the [`AllocFlavor::Rc`] flavor; for
    /// [`AllocFlavor::SimpleRef`] the chunk is marked `is_pinned`
    /// (no extra +1). Either way the caller receives a pointer to a
    /// freshly-initialized `T`.
    ///
    /// Drop-list entry registration: if `T: needs_drop`, a [`InnerDropEntry`]
    /// is appended to the back-stack so `T::drop` runs at chunk
    /// teardown.
    /// Bump-allocate room for a `T`, run `f` to initialize it, and
    /// commit the slot. Single-attempt fast path: if the current chunk
    /// has space, the entire bump-protect-call-commit sequence is
    /// performed inline. Slow paths (refill, oversized, post-closure
    /// chunk eviction) tail-call out to dedicated `#[cold]` helpers so
    /// the inlined image at the call site stays small.
    ///
    /// Reentrancy invariant — protective hold: before invoking `f` the
    /// chunk is incremented by one (via `rcs_issued` for Rc/Box,
    /// `arcs_issued` for Arc, or marked `is_pinned` for `SimpleRef`).
    /// On success, the +1 transfers to the smart pointer / pin-list;
    /// on closure panic, [`ProtectiveHold`] decrements it.
    ///
    /// Drop-list registration: if `T: needs_drop` and `flavor != Box`,
    /// an [`InnerDropEntry`] is appended to the back-stack so `T::drop`
    /// runs at chunk teardown. Box runs `drop_in_place` directly.
    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "fast-path bump body must inline into every public alloc/alloc_with/alloc_box/alloc_rc call site"
    )]
    #[cfg_attr(test, mutants::skip)]
    pub(super) fn try_alloc_slice_local_with<T, F, const PANIC: bool>(
        &self,
        len: usize,
        flavor: AllocFlavor,
        drop_fn: Option<unsafe fn(*mut u8, usize)>,
        init: F,
    ) -> Result<NonNull<[T]>, AllocError>
    where
        F: FnMut(usize, &mut MaybeUninit<T>),
    {
        self.impl_alloc_slice_local_with::<T, F, PANIC>(len, flavor, drop_fn, init)
    }

    /// Single source of truth for the local-flavor slice-with-init
    /// fast path. `PANIC=true` panics on chunk-allocation failure
    /// (via `panic_alloc()`); `PANIC=false` propagates `Err`. Each
    /// instantiation produces the same machine code as a hand-written
    /// try/panic pair would.
    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "slice fast path must inline into every public alloc_slice_*/try_alloc_slice_* call site so the PANIC const folds"
    )]
    fn impl_alloc_slice_local_with<T, F, const PANIC: bool>(
        &self,
        len: usize,
        flavor: AllocFlavor,
        drop_fn: Option<unsafe fn(*mut u8, usize)>,
        mut init: F,
    ) -> Result<NonNull<[T]>, AllocError>
    where
        F: FnMut(usize, &mut MaybeUninit<T>),
    {
        let Ok(layout) = Layout::array::<T>(len) else {
            if PANIC {
                panic_alloc();
            }
            return Err(AllocError);
        };
        if layout.align() >= MAX_SMART_PTR_ALIGN {
            if PANIC {
                panic_alloc();
            }
            return Err(AllocError);
        }
        // `Layout::array` already enforces `size_aligned <= isize::MAX`, so a
        // separate `check_isize_overflow` would be redundant.
        let entry_size = if drop_fn.is_some() && len != 0 {
            core::mem::size_of::<InnerDropEntry>()
        } else {
            0
        };
        if entry_size != 0 && len > u16::MAX as usize {
            if PANIC {
                panic_alloc();
            }
            return Err(AllocError);
        }
        // Route oversized requests to the cold one-shot path so we
        // never install a >64 KiB-payload chunk as `current_local`
        // (would break the chunk-recovery mask invariant).
        // Deferred to the slow path to keep the hot bump-fit
        // iteration free of a 2-level pointer chase through
        // `Arc<ChunkProvider>`.
        let bumped = layout.size().max(1);
        loop {
            let data_ptr = self.current_local.data_ptr.get();
            let drop_back_ptr = self.current_local.drop_back.get();
            let __fit = try_bump_fit(data_ptr, drop_back_ptr, layout.align().max(1), bumped, entry_size);
            if __fit.fits {
                let aligned_ptr = __fit.aligned_ptr;
                let end_ptr = __fit.end_ptr;
                let new_drop_back_ptr = __fit.new_drop_back_ptr;
                {
                    // SAFETY: bump-fit gate above implies non-stub slot ⇒ `current_local.chunk` is `Some`.
                    let chunk = unsafe { self.current_local.chunk.get().unwrap_unchecked() };
                    let ptr: *mut T = aligned_ptr.cast::<T>().as_ptr();

                    // Take the protective hold before running the
                    // `init` closure (which may reentrantly call
                    // back into the arena and evict this chunk).
                    match flavor {
                        AllocFlavor::SimpleRef => {
                            self.current_local_pinned.set(true);
                        }
                        AllocFlavor::Rc | AllocFlavor::Box => {
                            self.current_local.bump_smart_pointers_issued();
                        }
                    }
                    let hold = ProtectiveHold::<A> {
                        arena: self,
                        chunk,
                        flavor,
                    };

                    // Pre-advance the bump cursor and pre-write a
                    // noop drop entry into the reserved back-stack
                    // slot before invoking `init`. This prevents a
                    // reentrant alloc from inside `init` from
                    // overlapping our value reservation or claiming
                    // our drop-entry slot. The noop entry will be
                    // overwritten with the real drop shim if `init`
                    // succeeds; if `init` panics, the noop is
                    // harmless (replay calls noop on uninit memory,
                    // which is a no-op) and `SliceInitGuard` still
                    // drops the initialized prefix.
                    self.current_local.data_ptr.set(end_ptr);
                    // The `value_offset` / `len_u16` / chunk-pointer dereferences
                    // are only needed when we install a drop entry; hoist them
                    // inside that branch so non-drop slices skip the panic
                    // surface of the `u16` cast and an extra chunk-pointer load.
                    let (value_offset, len_u16) = if entry_size > 0 {
                        // SAFETY: refcount-positive — chunk held at LARGE inflation.
                        let payload_base_addr = unsafe { LocalChunk::<A>::data_ptr(chunk) }.as_ptr() as usize;
                        // Fast path is gated on `layout.size() <= max_normal_alloc <= MAX_CHUNK_BYTES`,
                        // so any aligned offset within the chunk payload is < 64 KiB and fits in `u16`.
                        // Same for `len_u16`: the `entry_size != 0 && len > u16::MAX` guard above
                        // already excluded `len > u16::MAX`.
                        // SAFETY: bounded by fast-path invariants (chunk payload < 64 KiB; len <= u16::MAX).
                        let value_offset = unsafe { u16::try_from((aligned_ptr.as_ptr() as usize) - payload_base_addr).unwrap_unchecked() };
                        // SAFETY: gated by `len > u16::MAX` check earlier in this function.
                        let len_u16 = unsafe { u16::try_from(len).unwrap_unchecked() };
                        self.current_local.drop_back.set(new_drop_back_ptr);
                        let entry_ptr: *mut InnerDropEntry = new_drop_back_ptr.cast::<InnerDropEntry>().as_ptr();
                        let noop_entry = InnerDropEntry::new(noop_drop_shim, value_offset, len_u16);
                        // SAFETY: `entry_ptr` is valid for one entry.
                        unsafe { core::ptr::write(entry_ptr, noop_entry) };
                        // SAFETY: protective hold keeps `chunk` alive.
                        unsafe { bump_local_drop_count(chunk) };
                        (value_offset, len_u16)
                    } else {
                        (0, 0)
                    };

                    let mut guard = SliceInitGuard { ptr, len: 0 };
                    // SAFETY: `ptr` is aligned, non-null, and covers `len`
                    // freshly-reserved slots in the chunk payload — i.e.
                    // exactly the layout of `[MaybeUninit<T>; len]`.
                    let slots: &mut [MaybeUninit<T>] = unsafe { core::slice::from_raw_parts_mut(ptr.cast::<MaybeUninit<T>>(), len) };
                    for (i, slot) in slots.iter_mut().enumerate() {
                        init(i, slot);
                        guard.len += 1;
                    }
                    core::mem::forget(guard);
                    core::mem::forget(hold);

                    // `init` succeeded: overwrite the noop entry with
                    // the real drop shim so the slice's elements get
                    // dropped on chunk teardown. For `Box` flavor we
                    // leave the entry as a noop because `Box<[T]>::drop`
                    // runs `drop_in_place` directly; `Box<[T]>::into_rc`
                    // retargets the entry to the real shim at conversion
                    // time.
                    //
                    // `entry_size > 0` already implies `drop_fn.is_some()
                    // && len != 0` (see the `entry_size` computation
                    // above), so this branch only needs to check the
                    // flavor gate and unwrap `drop_fn` without re-checking
                    // `len`.
                    if has_drop_entry(entry_size) && !matches!(flavor, AllocFlavor::Box) {
                        // SAFETY: `entry_size > 0` ⇔ `drop_fn.is_some() && len != 0`.
                        let drop_fn = unsafe { drop_fn.unwrap_unchecked() };
                        // Overwrite the noop drop shim with the real one;
                        // `value_offset` and `len_u16` were already written
                        // by the pre-closure noop entry and are unchanged,
                        // so we only update the 8-byte `drop_fn` pointer.
                        let entry_ptr: *mut InnerDropEntry = new_drop_back_ptr.cast::<InnerDropEntry>().as_ptr();
                        // SAFETY: `entry_ptr` references the
                        // pre-written noop entry. Local chunks are
                        // owner-thread exclusive
                        // (`LocalChunk: !Send`); Relaxed is
                        // sufficient (no cross-thread reader is
                        // possible).
                        unsafe { (*entry_ptr).store_drop_fn(drop_fn, Ordering::Relaxed) };
                    }
                    let _ = (value_offset, len_u16);

                    self.charge_alloc_stats(layout.size());
                    let fat = core::ptr::slice_from_raw_parts_mut(ptr, len);
                    // SAFETY: `fat` is non-null and covers the initialized elements.
                    return Ok(unsafe { NonNull::new_unchecked(fat) });
                }
            }
            // Route oversized requests to the cold one-shot path
            // so we never install a >64 KiB chunk as `current_local`.
            if size_exceeds_normal_alloc(layout.size(), self.provider.max_normal_alloc) {
                let r = self.try_alloc_slice_local_oversized_with::<T, F>(len, flavor, drop_fn, init);
                return if PANIC { Ok(expect_alloc(r)) } else { r };
            }
            let r = self.refill_local(worst_case_refill_for(layout, entry_size));
            if PANIC {
                expect_alloc(r);
            } else {
                r?;
            }
        }
    }

    /// Panicking sibling of [`Self::try_alloc_slice_local_with`].
    ///
    /// See [`Self::alloc_inner_value_or_panic`] for the design rationale.
    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "slice fast path must inline into every public panicking alloc_slice_* call site"
    )]
    #[cfg_attr(test, mutants::skip)]
    pub(super) fn alloc_slice_local_with_or_panic<T, F>(
        &self,
        len: usize,
        flavor: AllocFlavor,
        drop_fn: Option<unsafe fn(*mut u8, usize)>,
        init: F,
    ) -> NonNull<[T]>
    where
        F: FnMut(usize, &mut MaybeUninit<T>),
    {
        expect_alloc(self.impl_alloc_slice_local_with::<T, F, true>(len, flavor, drop_fn, init))
    }

    /// Specialized variant of [`Self::try_alloc_slice_local_with`] for types
    /// that do not need to be dropped.
    ///
    /// Mirrors [`Self::try_alloc_slice_local_with`] but elides every
    /// drop-entry-related concern: no `drop_fn` parameter, no `entry_size`
    /// reservation, no `value_offset`/`u16` length checks, no `drop_back`
    /// store on success, and no `InnerDropEntry` install. Callers are
    /// responsible for ensuring `!core::mem::needs_drop::<T>()`.
    #[inline(always)]
    #[expect(clippy::inline_always, reason = "see method-level comment")]
    #[cfg_attr(test, mutants::skip)]
    pub(super) fn try_alloc_slice_local_no_drop_with<T, F, const PANIC: bool>(
        &self,
        len: usize,
        flavor: AllocFlavor,
        mut init: F,
    ) -> Result<NonNull<[T]>, AllocError>
    where
        F: FnMut(usize, &mut MaybeUninit<T>),
    {
        debug_assert!(
            !core::mem::needs_drop::<T>(),
            "try_alloc_slice_local_no_drop_with requires T: !Drop"
        );
        let Ok(layout) = Layout::array::<T>(len) else {
            if PANIC {
                panic_alloc();
            }
            return Err(AllocError);
        };
        // SimpleRef returns `&mut [T]`, so the cap is the
        // chunk-recovery limit (`CHUNK_ALIGN`); Box/Rc need the
        // tighter smart-pointer cap so `from_value_ptr` round-trips.
        let align_cap = match flavor {
            AllocFlavor::SimpleRef => crate::internal::constants::CHUNK_ALIGN,
            AllocFlavor::Box | AllocFlavor::Rc => MAX_SMART_PTR_ALIGN,
        };
        if layout.align() >= align_cap {
            if PANIC {
                panic_alloc();
            }
            return Err(AllocError);
        }
        // `Layout::array` enforces `size_aligned <= isize::MAX`.
        // The `max_normal_alloc` check is deferred to the cold slow
        // path to keep the hot bump-fit probe free of a 2-level
        // pointer chase through `Arc<ChunkProvider>`.
        let bumped = layout.size().max(1);

        // Fast path: single bump-check; on miss, take the cold refill loop.
        let data_ptr = self.current_local.data_ptr.get();
        let drop_back_ptr = self.current_local.drop_back.get();
        let __fit = try_bump_fit(
            data_ptr,
            drop_back_ptr,
            layout.align().max(1),
            bumped,
            0, // no drop entry for !needs_drop
        );
        if !__fit.fits {
            let r = self.try_alloc_slice_local_no_drop_with_slow::<T, F>(len, flavor, init, layout, bumped);
            return if PANIC { Ok(expect_alloc(r)) } else { r };
        }
        let aligned_ptr = __fit.aligned_ptr;
        let end_ptr = __fit.end_ptr;

        // SAFETY: chunk-present invariant — fast-path gate above
        // implies a real chunk is loaded.
        let chunk = unsafe { self.current_local.chunk.get().unwrap_unchecked() };
        let ptr: *mut T = aligned_ptr.cast::<T>().as_ptr();

        match flavor {
            AllocFlavor::SimpleRef => {
                self.current_local_pinned.set(true);
            }
            AllocFlavor::Rc | AllocFlavor::Box => {
                self.current_local.bump_smart_pointers_issued();
            }
        }
        let hold = ProtectiveHold::<A> {
            arena: self,
            chunk,
            flavor,
        };

        // Pre-advance the bump cursor before invoking the user
        // closure: a reentrant `alloc_*` from inside `init` would
        // otherwise reread the un-advanced `data_ptr` and overlap
        // our reservation.
        self.current_local.data_ptr.set(end_ptr);

        // No SliceInitGuard: the `T: !needs_drop` precondition means
        // there's nothing to drop on `init` panic, so the guard's
        // `len += 1` store per element would be dead weight. The
        // `hold`'s drop still releases the protective +1 if `init`
        // unwinds.
        let slots_ptr: *mut MaybeUninit<T> = ptr.cast::<MaybeUninit<T>>();
        // Plain index loop (rather than `slots.iter_mut().enumerate()`):
        // when `init` collapses to a trivial load/store (e.g. cloning a
        // `Copy` primitive), LLVM's loop-vectorizer is much more likely
        // to fold the body into AVX moves through this shape than
        // through the iterator chain.
        for i in 0..len {
            // SAFETY: `slots_ptr` is non-null, aligned, and covers
            // `len` freshly-reserved `MaybeUninit<T>` slots; `i < len`.
            let slot = unsafe { &mut *slots_ptr.add(i) };
            init(i, slot);
        }
        core::mem::forget(hold);

        self.charge_alloc_stats(layout.size());
        let fat = core::ptr::slice_from_raw_parts_mut(ptr, len);
        // SAFETY: `fat` is non-null and covers the initialized elements.
        Ok(unsafe { NonNull::new_unchecked(fat) })
    }

    /// Cold tail of [`Self::try_alloc_slice_local_no_drop_with`] —
    /// refill loop for when the fast bump-check misses.
    #[cold]
    pub(super) fn try_alloc_slice_local_no_drop_with_slow<T, F>(
        &self,
        len: usize,
        flavor: AllocFlavor,
        init: F,
        layout: Layout,
        bumped: usize,
    ) -> Result<NonNull<[T]>, AllocError>
    where
        F: FnMut(usize, &mut MaybeUninit<T>),
    {
        let _ = bumped;
        // Route oversized requests to the one-shot path (deferred
        // from the hot path to avoid a provider pointer chase).
        if layout.size() > self.provider.max_normal_alloc {
            return self.try_alloc_slice_local_oversized_with::<T, F>(len, flavor, None, init);
        }
        self.refill_local(compute_worst_case_size(layout, false))?;
        // `refill_local` post-condition: the refreshed chunk fits the request,
        // so the recursive fast-path call below cannot miss the bump-fit gate.
        self.try_alloc_slice_local_no_drop_with::<T, F, false>(len, flavor, init)
    }

    /// Fast path for `T: Copy` slice allocation in a local-flavor chunk.
    ///
    /// Mirrors [`Self::try_alloc_slice_local_with`] but skips the per-element
    /// closure loop in favor of a single `ptr::copy_nonoverlapping`. Because
    /// `T: Copy` implies `!needs_drop::<T>()`, no drop entry is ever installed.
    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "slice fast path must inline into every public alloc_slice_* call site"
    )]
    #[cfg_attr(test, mutants::skip)]
    pub(super) fn try_alloc_slice_local_copy<T: Copy, const PANIC: bool>(
        &self,
        src: &[T],
        flavor: AllocFlavor,
    ) -> Result<NonNull<[T]>, AllocError> {
        self.impl_alloc_slice_local_copy::<T, PANIC>(src, flavor)
    }

    /// Single source of truth for the local-flavor `T: Copy` slice
    /// fast path. `PANIC=true` panics on chunk-allocation failure;
    /// `PANIC=false` propagates `Err`.
    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "slice fast path must inline into every public alloc_slice_copy* / try_alloc_slice_copy* call site so the PANIC const folds"
    )]
    fn impl_alloc_slice_local_copy<T: Copy, const PANIC: bool>(&self, src: &[T], flavor: AllocFlavor) -> Result<NonNull<[T]>, AllocError> {
        let len = src.len();
        // SAFETY: `src: &[T]`'s safety contract already requires
        // `len * size_of::<T>() <= isize::MAX`, which is exactly the
        // bound `Layout::array::<T>(len)` checks. So this never fails.
        let layout = unsafe { Layout::array::<T>(len).unwrap_unchecked() };
        // The Copy path doesn't reserve a trailing drop entry, so the
        // half-chunk-align constraint only applies up to (but not
        // including) `CHUNK_ALIGN` itself — alignments equal to
        // `CHUNK_ALIGN` would let an in-payload pointer mask back to
        // its own offset = 0 and confuse the chunk-recovery header
        // mask. Drop-aware siblings use a stricter cap because they
        // also reserve trailing drop-list entries.
        if layout.align() >= crate::internal::constants::CHUNK_ALIGN {
            if PANIC {
                panic_alloc();
            }
            return Err(AllocError);
        }
        let bumped = layout.size().max(1);

        // Fast path: single-branch fit check; on miss, cold slow path
        // handles both oversized routing and refill-retry. The
        // `max_normal_alloc` check is deferred to the slow path so the
        // hot loop avoids a 2-level pointer chase through `Arc<ChunkProvider>`.
        let data_ptr = self.current_local.data_ptr.get();
        let drop_back_ptr = self.current_local.drop_back.get();
        let __fit = try_bump_fit(
            data_ptr,
            drop_back_ptr,
            layout.align().max(1),
            bumped,
            0, // Copy path doesn't reserve a drop entry
        );
        if __fit.fits {
            let aligned_ptr = __fit.aligned_ptr;
            let end_ptr = __fit.end_ptr;

            // `Copy` cannot panic or reenter, so we skip the protective-hold
            // guard and only bump the per-flavor accounting that the chunk's
            // smart-pointer container expects.
            let ptr: *mut T = aligned_ptr.cast::<T>().as_ptr();
            match flavor {
                AllocFlavor::SimpleRef => {
                    self.current_local_pinned.set(true);
                }
                AllocFlavor::Rc | AllocFlavor::Box => {
                    self.current_local.bump_smart_pointers_issued();
                }
            }

            // Publish the new bump cursor BEFORE the memcpy so the next
            // iteration's load can satisfy via store-forwarding while the
            // copy stores drain through the store buffer. The bump-fit
            // check above guarantees the entire `[ptr, ptr + len)`
            // range still lies inside the chunk's payload, so observers
            // (other arena APIs invoked re-entrantly from a Drop, etc.)
            // cannot reach into uninitialized memory. `Copy`
            // initialization itself cannot reenter the arena, so the
            // order of "publish cursor" and "fill buffer" is
            // semantically interchangeable on the single owning thread.
            self.current_local.data_ptr.set(end_ptr);

            // SAFETY: `src` and the reserved range are non-overlapping; both are
            // valid for `len` elements; alignment is satisfied by `aligned_addr`.
            unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), ptr, len) };

            self.charge_alloc_stats(layout.size());
            let fat = core::ptr::slice_from_raw_parts_mut(ptr, len);
            // SAFETY: `fat` is non-null and covers the initialized elements.
            return Ok(unsafe { NonNull::new_unchecked(fat) });
        }
        let r = self.alloc_slice_local_copy_slow::<T>(src, flavor, layout, bumped);
        if PANIC { Ok(expect_alloc(r)) } else { r }
    }

    /// Cold refill-and-retry path for [`Self::impl_alloc_slice_local_copy`].
    /// Marked `#[cold] #[inline(never)]` so the hot fast path stays
    /// branch-light: the slow refill loop and its retry live entirely
    /// in this function's body.
    #[cold]
    #[inline(never)]
    fn alloc_slice_local_copy_slow<T: Copy>(
        &self,
        src: &[T],
        flavor: AllocFlavor,
        layout: Layout,
        bumped: usize,
    ) -> Result<NonNull<[T]>, AllocError> {
        let len = src.len();
        // Route oversized allocations to the one-shot path. This check
        // was deferred from the hot path to avoid a 2-level pointer
        // chase through `Arc<ChunkProvider>` on every iteration.
        if layout.size() > self.provider.max_normal_alloc {
            return self.try_alloc_slice_local_oversized_with::<T, _>(len, flavor, None, |i, slot| {
                slot.write(src[i]);
            });
        }
        self.refill_local(compute_worst_case_size(layout, false))?;
        let data_ptr = self.current_local.data_ptr.get();
        let drop_back_ptr = self.current_local.drop_back.get();
        let __fit = try_bump_fit(data_ptr, drop_back_ptr, layout.align().max(1), bumped, 0);
        // `refill_local` acquires a chunk of at least
        // `compute_worst_case_size(layout, false) = size + align`
        // bytes. Chunks are `CHUNK_ALIGN`-aligned, so the bump
        // cursor at chunk start is already aligned to any
        // `layout.align() <= CHUNK_ALIGN`; the alignment padding
        // term is therefore zero and `aligned + bumped <=
        // capacity` always holds.
        debug_assert!(__fit.fits, "refill_local guarantees a fitting chunk for Copy slice fast path");
        let aligned_ptr = __fit.aligned_ptr;
        let end_ptr = __fit.end_ptr;
        let ptr: *mut T = aligned_ptr.cast::<T>().as_ptr();
        match flavor {
            AllocFlavor::SimpleRef => {
                self.current_local_pinned.set(true);
            }
            AllocFlavor::Rc | AllocFlavor::Box => {
                self.current_local.bump_smart_pointers_issued();
            }
        }
        self.current_local.data_ptr.set(end_ptr);
        // SAFETY: same invariants as the fast path.
        unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), ptr, len) };
        self.charge_alloc_stats(layout.size());
        let fat = core::ptr::slice_from_raw_parts_mut(ptr, len);
        // SAFETY: `fat` is non-null and covers the initialized elements.
        Ok(unsafe { NonNull::new_unchecked(fat) })
    }

    /// Panicking sibling of [`Self::try_alloc_slice_local_copy`].
    ///
    /// Returns `NonNull<[T]>` directly (no `Result` wrapper). On
    /// allocation failure this calls [`panic_alloc`] instead of
    /// propagating an error. Public panicking entry points (e.g.
    /// `alloc_slice_copy`) call this variant so the bench/hot-loop
    /// caller does not see a dead niche-check on a
    /// `Result<NonNull<_>, _>` return value, which would otherwise
    /// add a fused `test rax, rax / je` to every iteration. The body
    /// is intentionally a near-duplicate of `try_alloc_slice_local_copy`;
    /// keeping them as separate concrete functions (rather than a
    /// const-generic over a `FALLIBLE` flag) is what allows the
    /// compiler to elide the `Result` discriminant entirely on the
    /// panicking path.
    ///
    /// Always installs as a simple-reference allocation: the only
    /// public caller is [`Self::alloc_slice_copy`].
    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "slice fast path must inline into every public alloc_slice_* panicking call site"
    )]
    #[cfg_attr(test, mutants::skip)]
    pub(super) fn alloc_slice_local_copy_or_panic<T: Copy>(&self, src: &[T]) -> NonNull<[T]> {
        expect_alloc(self.impl_alloc_slice_local_copy::<T, true>(src, AllocFlavor::SimpleRef))
    }

    /// Fast path for `T: Copy + Send + Sync` slice allocation in a shared-flavor chunk.
    ///
    /// Mirrors [`Self::try_alloc_slice_shared_with`] but uses a single
    /// `ptr::copy_nonoverlapping`. Because `T: Copy` implies `!needs_drop::<T>()`,
    /// no drop entry is ever installed.
    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "slice fast path must inline into every public alloc_slice_* call site"
    )]
    #[cfg_attr(test, mutants::skip)]
    pub(super) fn try_alloc_slice_shared_copy<T: Copy + Send + Sync, const PANIC: bool>(
        &self,
        src: &[T],
    ) -> Result<NonNull<[T]>, AllocError> {
        let len = src.len();
        // SAFETY: `src: &[T]`'s safety contract already bounds
        // `len * size_of::<T>() <= isize::MAX`, which is what
        // `Layout::array::<T>(len)` would check.
        let layout = unsafe { Layout::array::<T>(len).unwrap_unchecked() };
        // See `try_alloc_slice_local_copy` for the rationale on the
        // looser `CHUNK_ALIGN` cap (vs `MAX_SMART_PTR_ALIGN` for the
        // Drop-aware paths).
        if layout.align() >= crate::internal::constants::CHUNK_ALIGN {
            if PANIC {
                panic_alloc();
            }
            return Err(AllocError);
        }
        let bumped = layout.size().max(1);
        // Fast path: single-branch fit check; on miss, cold slow path
        // handles both oversized routing and refill-retry. The
        // `max_normal_alloc` check is deferred to the slow path so the
        // hot loop avoids a 2-level pointer chase through `Arc<ChunkProvider>`.
        let data_ptr = self.current_shared.data_ptr.get();
        let drop_back_ptr = self.current_shared.drop_back.get();
        let __fit = try_bump_fit(data_ptr, drop_back_ptr, layout.align().max(1), bumped, 0);
        if __fit.fits {
            let aligned_ptr = __fit.aligned_ptr;
            let end_ptr = __fit.end_ptr;
            let ptr: *mut T = aligned_ptr.cast::<T>().as_ptr();

            // `Copy` initialization cannot panic or reenter, so no hold is needed.
            self.current_shared.bump_smart_pointers_issued();

            // Publish the new bump cursor BEFORE the memcpy so
            // the next iteration's `data_ptr.get()` load can
            // satisfy via store-forwarding without waiting for
            // the memcpy stores to drain (mirror of
            // `try_alloc_slice_local_copy`).
            self.current_shared.data_ptr.set(end_ptr);

            // SAFETY: `src` and the reserved range are non-overlapping; both are
            // valid for `len` elements; alignment is satisfied by `aligned_addr`.
            unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), ptr, len) };

            self.charge_alloc_stats(layout.size());
            let fat = core::ptr::slice_from_raw_parts_mut(ptr, len);
            // SAFETY: `fat` is non-null and covers the initialized elements.
            return Ok(unsafe { NonNull::new_unchecked(fat) });
        }
        self.alloc_slice_shared_copy_slow::<T>(src, layout, bumped)
    }

    /// Cold refill-and-retry path for [`Self::try_alloc_slice_shared_copy`].
    /// See [`Self::alloc_slice_local_copy_slow`] for the design rationale.
    #[cold]
    #[inline(never)]
    fn alloc_slice_shared_copy_slow<T: Copy + Send + Sync>(
        &self,
        src: &[T],
        layout: Layout,
        bumped: usize,
    ) -> Result<NonNull<[T]>, AllocError> {
        let len = src.len();
        // Route oversized allocations to the one-shot path. Deferred
        // from the hot path (see `try_alloc_slice_shared_copy`).
        if layout.size() > self.provider.max_normal_alloc {
            return self.try_alloc_slice_shared_oversized_with::<T, _>(len, None, |i, slot| {
                slot.write(src[i]);
            });
        }
        self.refill_shared(compute_worst_case_size(layout, false))?;
        let data_ptr = self.current_shared.data_ptr.get();
        let drop_back_ptr = self.current_shared.drop_back.get();
        let __fit = try_bump_fit(data_ptr, drop_back_ptr, layout.align().max(1), bumped, 0);
        // See `alloc_slice_local_copy_slow` for why the fit is guaranteed
        // after a successful refill.
        debug_assert!(__fit.fits, "refill_shared guarantees a fitting chunk for Copy slice fast path");
        let aligned_ptr = __fit.aligned_ptr;
        let end_ptr = __fit.end_ptr;
        let ptr: *mut T = aligned_ptr.cast::<T>().as_ptr();
        self.current_shared.bump_smart_pointers_issued();
        self.current_shared.data_ptr.set(end_ptr);
        // SAFETY: same invariants as the fast path.
        unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), ptr, len) };
        self.charge_alloc_stats(layout.size());
        let fat = core::ptr::slice_from_raw_parts_mut(ptr, len);
        // SAFETY: `fat` is non-null and covers the initialized elements.
        Ok(unsafe { NonNull::new_unchecked(fat) })
    }

    // Shared slice mirror of `try_alloc_slice_local_with`.
    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "shared slice fast path must inline into every public alloc_*_arc/try_alloc_*_arc call site so PANIC folds"
    )]
    pub(super) fn try_alloc_slice_shared_with<T, F, const PANIC: bool>(
        &self,
        len: usize,
        drop_fn: Option<unsafe fn(*mut u8, usize)>,
        init: F,
    ) -> Result<NonNull<[T]>, AllocError>
    where
        F: FnMut(usize, &mut MaybeUninit<T>),
    {
        self.impl_alloc_slice_shared_with::<T, F, PANIC>(len, drop_fn, init)
    }

    /// Single source of truth for the shared-flavor slice-with-init
    /// fast path. `PANIC=true` panics on chunk-allocation failure;
    /// `PANIC=false` propagates `Err`.
    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "shared slice fast path must inline into every public alloc_*_arc/try_alloc_*_arc call site so the PANIC const folds"
    )]
    fn impl_alloc_slice_shared_with<T, F, const PANIC: bool>(
        &self,
        len: usize,
        drop_fn: Option<unsafe fn(*mut u8, usize)>,
        mut init: F,
    ) -> Result<NonNull<[T]>, AllocError>
    where
        F: FnMut(usize, &mut MaybeUninit<T>),
    {
        let Ok(layout) = Layout::array::<T>(len) else {
            if PANIC {
                panic_alloc();
            }
            return Err(AllocError);
        };
        if layout.align() >= MAX_SMART_PTR_ALIGN {
            if PANIC {
                panic_alloc();
            }
            return Err(AllocError);
        }
        // `Layout::array` enforces `size_aligned <= isize::MAX`.
        let entry_size = if drop_fn.is_some() && len != 0 {
            core::mem::size_of::<InnerDropEntry>()
        } else {
            0
        };
        if entry_size != 0 && len > u16::MAX as usize {
            if PANIC {
                panic_alloc();
            }
            return Err(AllocError);
        }
        // Route oversized requests to the cold one-shot path.
        // Deferred to the slow path to keep the hot bump-fit
        // iteration free of a 2-level pointer chase through
        // `Arc<ChunkProvider>`.
        let bumped = layout.size().max(1);
        loop {
            let data_ptr = self.current_shared.data_ptr.get();
            let drop_back_ptr = self.current_shared.drop_back.get();
            let __fit = try_bump_fit(data_ptr, drop_back_ptr, layout.align().max(1), bumped, entry_size);
            if __fit.fits {
                let aligned_ptr = __fit.aligned_ptr;
                let end_ptr = __fit.end_ptr;
                let new_drop_back_ptr = __fit.new_drop_back_ptr;
                {
                    // SAFETY: bump-fit gate above implies non-stub slot ⇒ `current_shared.chunk` is `Some`.
                    let chunk = unsafe { self.current_shared.chunk.get().unwrap_unchecked() };
                    let ptr: *mut T = aligned_ptr.cast::<T>().as_ptr();

                    // Account before `init` so reentrant refill preserves this +1.
                    self.current_shared.bump_smart_pointers_issued();
                    let hold = SharedArcsIssuedHold { arena: self, chunk };

                    // Pre-advance the bump cursor and pre-write a
                    // noop drop entry into the reserved back-stack
                    // slot before `init` so a reentrant alloc
                    // cannot overlap our value or our drop-entry
                    // slot. The noop is overwritten with the real
                    // shim if `init` succeeds; if `init` panics,
                    // the noop is harmless and `SliceInitGuard`
                    // addresses lie in the chunk payload (gated
                    // above).
                    self.current_shared.data_ptr.set(end_ptr);
                    // `value_offset` / `len_u16` and the chunk-pointer dereferences
                    // are only needed when we install a drop entry; hoist them
                    // inside that branch so non-drop slices skip the panic
                    // surface of the `u16` casts and an extra chunk-pointer load.
                    let (value_offset, len_u16) = if has_drop_entry(entry_size) {
                        // SAFETY: refcount-positive — chunk held at LARGE inflation.
                        let payload_base_addr = unsafe { SharedChunk::<A>::data_ptr(chunk) }.as_ptr() as usize;
                        // Bounded by fast-path invariants: chunk payload < 64 KiB, so the offset
                        // fits in `u16`; the `entry_size != 0 && len > u16::MAX` guard above
                        // already excluded large `len`.
                        // SAFETY: see comment above.
                        let value_offset = unsafe { u16::try_from((aligned_ptr.as_ptr() as usize) - payload_base_addr).unwrap_unchecked() };
                        // SAFETY: gated by `len > u16::MAX` check earlier in this function.
                        let len_u16 = unsafe { u16::try_from(len).unwrap_unchecked() };
                        self.current_shared.drop_back.set(new_drop_back_ptr);
                        let entry_ptr: *mut InnerDropEntry = new_drop_back_ptr.cast::<InnerDropEntry>().as_ptr();
                        let noop_entry = InnerDropEntry::new(noop_drop_shim, value_offset, len_u16);
                        // SAFETY: `entry_ptr` is valid for one entry.
                        unsafe { core::ptr::write(entry_ptr, noop_entry) };
                        // SAFETY: refcount-positive — chunk is live.
                        unsafe { bump_shared_drop_count(chunk) };
                        (value_offset, len_u16)
                    } else {
                        (0, 0)
                    };

                    let mut guard = SliceInitGuard { ptr, len: 0 };
                    // SAFETY: `ptr` is aligned, non-null, and covers `len`
                    // freshly-reserved slots in the chunk payload.
                    let slots: &mut [MaybeUninit<T>] = unsafe { core::slice::from_raw_parts_mut(ptr.cast::<MaybeUninit<T>>(), len) };
                    for (i, slot) in slots.iter_mut().enumerate() {
                        init(i, slot);
                        guard.len += 1;
                    }
                    core::mem::forget(guard);
                    core::mem::forget(hold);

                    // `init` succeeded: overwrite the noop entry
                    // with the real drop shim.
                    //
                    // `entry_size > 0` already implies `drop_fn.is_some()
                    // && len != 0`, so we can unwrap `drop_fn` without a
                    // separate `len` check.
                    if has_drop_entry(entry_size) {
                        // SAFETY: `entry_size > 0` ⇔ `drop_fn.is_some() && len != 0`.
                        let drop_fn = unsafe { drop_fn.unwrap_unchecked() };
                        // Overwrite the noop drop shim with the real one;
                        // `value_offset` and `len_u16` were already written
                        // by the pre-closure noop entry and are unchanged,
                        // so we only update the 8-byte `drop_fn` pointer.
                        let entry_ptr: *mut InnerDropEntry = new_drop_back_ptr.cast::<InnerDropEntry>().as_ptr();
                        // SAFETY: `entry_ptr` references the
                        // pre-written noop entry. The `Arc<[T]>` for
                        // this allocation has not been returned to
                        // the caller yet, so no other thread can
                        // observe this slot's `drop_fn`. Relaxed is
                        // sufficient: the eventual `Arc::drop`'s
                        // Release on `refcount` carries the new
                        // `drop_fn` to any subsequent `replay_drops`
                        // reader.
                        unsafe { (*entry_ptr).store_drop_fn(drop_fn, Ordering::Relaxed) };
                        let _ = (value_offset, len_u16);
                    }

                    self.charge_alloc_stats(layout.size());
                    let fat = core::ptr::slice_from_raw_parts_mut(ptr, len);
                    // SAFETY: `fat` is non-null and covers the initialized elements.
                    return Ok(unsafe { NonNull::new_unchecked(fat) });
                }
            }
            // Route oversized requests to the cold one-shot path
            // so we never install a >64 KiB chunk as `current_shared`.
            if size_exceeds_normal_alloc(layout.size(), self.provider.max_normal_alloc) {
                let r = self.try_alloc_slice_shared_oversized_with::<T, F>(len, drop_fn, init);
                return if PANIC { Ok(expect_alloc(r)) } else { r };
            }
            let r = self.refill_shared(worst_case_refill_for(layout, entry_size));
            if PANIC {
                expect_alloc(r);
            } else {
                r?;
            }
        }
    }

    /// Panicking sibling of [`Self::try_alloc_slice_shared_with`].
    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "shared slice fast path must inline into every public panicking alloc_*_arc call site"
    )]
    pub(super) fn alloc_slice_shared_with_or_panic<T, F>(
        &self,
        len: usize,
        drop_fn: Option<unsafe fn(*mut u8, usize)>,
        init: F,
    ) -> NonNull<[T]>
    where
        F: FnMut(usize, &mut MaybeUninit<T>),
    {
        expect_alloc(self.impl_alloc_slice_shared_with::<T, F, true>(len, drop_fn, init))
    }

    /// Specialized variant of [`Self::try_alloc_slice_shared_with`] for types
    /// that do not need to be dropped.
    ///
    /// Mirrors [`Self::try_alloc_slice_shared_with`] but elides every
    /// drop-entry-related concern: no `drop_fn` parameter, no `entry_size`
    /// reservation, no `value_offset`/`u16` length checks, no `drop_back`
    /// store on success, and no `InnerDropEntry` install. Callers are
    /// responsible for ensuring `!core::mem::needs_drop::<T>()`.
    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "slice fast path must inline into every public alloc_slice_* call site"
    )]
    #[cfg_attr(test, mutants::skip)]
    pub(super) fn try_alloc_slice_shared_no_drop_with<T, F, const PANIC: bool>(
        &self,
        len: usize,
        mut init: F,
    ) -> Result<NonNull<[T]>, AllocError>
    where
        F: FnMut(usize, &mut MaybeUninit<T>),
    {
        debug_assert!(
            !core::mem::needs_drop::<T>(),
            "try_alloc_slice_shared_no_drop_with requires T: !Drop"
        );
        let Ok(layout) = Layout::array::<T>(len) else {
            if PANIC {
                panic_alloc();
            }
            return Err(AllocError);
        };
        if layout.align() >= MAX_SMART_PTR_ALIGN {
            if PANIC {
                panic_alloc();
            }
            return Err(AllocError);
        }
        // `Layout::array` enforces `size_aligned <= isize::MAX`.
        // The `max_normal_alloc` check is deferred to the cold refill
        // branch to keep the hot bump-fit probe free of a 2-level
        // pointer chase through `Arc<ChunkProvider>`.
        let bumped = layout.size().max(1);
        loop {
            let data_ptr = self.current_shared.data_ptr.get();
            let drop_back_ptr = self.current_shared.drop_back.get();
            let __fit = try_bump_fit(data_ptr, drop_back_ptr, layout.align().max(1), bumped, 0);
            if __fit.fits {
                let aligned_ptr = __fit.aligned_ptr;
                let end_ptr = __fit.end_ptr;
                {
                    // SAFETY: bump-fit gate above implies non-stub slot ⇒ `current_shared.chunk` is `Some`.
                    let chunk = unsafe { self.current_shared.chunk.get().unwrap_unchecked() };
                    let ptr: *mut T = aligned_ptr.cast::<T>().as_ptr();

                    // Account before `init` so reentrant refill preserves this +1.
                    self.current_shared.bump_smart_pointers_issued();
                    let hold = SharedArcsIssuedHold { arena: self, chunk };

                    // Pre-advance the bump cursor before `init` so a
                    // reentrant alloc cannot overlap our reservation.
                    self.current_shared.data_ptr.set(end_ptr);

                    // No SliceInitGuard: the `T: !needs_drop` precondition
                    // makes per-element panic cleanup a no-op.
                    // SAFETY: `ptr` is aligned, non-null, and covers `len`
                    // freshly-reserved slots in the chunk payload.
                    let slots: &mut [MaybeUninit<T>] = unsafe { core::slice::from_raw_parts_mut(ptr.cast::<MaybeUninit<T>>(), len) };
                    for (i, slot) in slots.iter_mut().enumerate() {
                        init(i, slot);
                    }
                    core::mem::forget(hold);

                    // `slot.data_ptr` was pre-advanced before the closure ran.

                    self.charge_alloc_stats(layout.size());
                    let fat = core::ptr::slice_from_raw_parts_mut(ptr, len);
                    // SAFETY: `fat` is non-null and covers the initialized elements.
                    return Ok(unsafe { NonNull::new_unchecked(fat) });
                }
            }
            // Route oversized requests to the cold one-shot path
            // (deferred from above the loop to avoid a provider pointer chase).
            if size_exceeds_normal_alloc(layout.size(), self.provider.max_normal_alloc) {
                let r = self.try_alloc_slice_shared_oversized_with::<T, F>(len, None, init);
                return if PANIC { Ok(expect_alloc(r)) } else { r };
            }
            let r = self.refill_shared(compute_worst_case_size(layout, false));
            if PANIC {
                expect_alloc(r);
            } else {
                r?;
            }
        }
    }
}

// Auto-drop dispatch helpers. Each consolidates the
// `const { needs_drop::<T>() }` branch that would otherwise be
// duplicated at every public slice-alloc call site. The branch is
// resolved at monomorphization, so each instantiation expands to the
// same code that hand-written if/else versions produced.
impl<A: Allocator + Clone> Arena<A> {
    /// `clone`-init dispatch for local-flavor slices.
    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "must inline so the const PANIC flag propagates into the inner fast paths"
    )]
    pub(super) fn try_alloc_slice_local_clone_inner<T: Clone, const PANIC: bool>(
        &self,
        slice: &[T],
        flavor: AllocFlavor,
    ) -> Result<NonNull<[T]>, AllocError> {
        let len = slice.len();
        let init = |i: usize, dst: &mut MaybeUninit<T>| {
            // SAFETY: destination is reserved; `i` is in `0..len` where `len == slice.len()`.
            dst.write(unsafe { slice.get_unchecked(i) }.clone());
        };
        if const { core::mem::needs_drop::<T>() } {
            self.try_alloc_slice_local_with::<_, _, PANIC>(len, flavor, super::drop_fn_for_slice::<T>(), init)
        } else {
            self.try_alloc_slice_local_no_drop_with::<_, _, PANIC>(len, flavor, init)
        }
    }

    /// `fill_with`-init dispatch for local-flavor slices.
    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "must inline so the const PANIC flag propagates into the inner fast paths"
    )]
    pub(super) fn try_alloc_slice_local_fill_with_inner<T, F: FnMut(usize) -> T, const PANIC: bool>(
        &self,
        len: usize,
        flavor: AllocFlavor,
        mut f: F,
    ) -> Result<NonNull<[T]>, AllocError> {
        let init = |i: usize, dst: &mut MaybeUninit<T>| {
            dst.write(f(i));
        };
        if const { core::mem::needs_drop::<T>() } {
            self.try_alloc_slice_local_with::<_, _, PANIC>(len, flavor, super::drop_fn_for_slice::<T>(), init)
        } else {
            self.try_alloc_slice_local_no_drop_with::<_, _, PANIC>(len, flavor, init)
        }
    }

    /// `fill_iter`-init dispatch for local-flavor slices.
    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "must inline so the const PANIC flag propagates into the inner fast paths"
    )]
    pub(super) fn try_alloc_slice_local_fill_iter_inner<T, I, const PANIC: bool>(
        &self,
        iter: I,
        flavor: AllocFlavor,
    ) -> Result<NonNull<[T]>, AllocError>
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
    {
        let mut iter = iter.into_iter();
        let len = iter.len();
        let init = |_: usize, dst: &mut MaybeUninit<T>| {
            dst.write(
                iter.next()
                    .expect("caller violated ExactSizeIterator contract: iter.len() reported more elements than iter.next() yields"),
            );
        };
        if const { core::mem::needs_drop::<T>() } {
            self.try_alloc_slice_local_with::<_, _, PANIC>(len, flavor, super::drop_fn_for_slice::<T>(), init)
        } else {
            self.try_alloc_slice_local_no_drop_with::<_, _, PANIC>(len, flavor, init)
        }
    }

    /// `clone`-init dispatch for shared-flavor slices (used by `Arc`).
    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "must inline so the const PANIC flag propagates into the inner fast paths"
    )]
    pub(super) fn try_alloc_slice_shared_clone_inner<T: Clone, const PANIC: bool>(&self, slice: &[T]) -> Result<NonNull<[T]>, AllocError> {
        let len = slice.len();
        let init = |i: usize, dst: &mut MaybeUninit<T>| {
            // SAFETY: destination is reserved; `i` is in `0..len` where `len == slice.len()`.
            dst.write(unsafe { slice.get_unchecked(i) }.clone());
        };
        if const { core::mem::needs_drop::<T>() } {
            self.try_alloc_slice_shared_with::<_, _, PANIC>(len, super::drop_fn_for_slice::<T>(), init)
        } else {
            self.try_alloc_slice_shared_no_drop_with::<_, _, PANIC>(len, init)
        }
    }

    /// `fill_with`-init dispatch for shared-flavor slices.
    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "must inline so the const PANIC flag propagates into the inner fast paths"
    )]
    pub(super) fn try_alloc_slice_shared_fill_with_inner<T, F: FnMut(usize) -> T, const PANIC: bool>(
        &self,
        len: usize,
        mut f: F,
    ) -> Result<NonNull<[T]>, AllocError> {
        let init = |i: usize, dst: &mut MaybeUninit<T>| {
            dst.write(f(i));
        };
        if const { core::mem::needs_drop::<T>() } {
            self.try_alloc_slice_shared_with::<_, _, PANIC>(len, super::drop_fn_for_slice::<T>(), init)
        } else {
            self.try_alloc_slice_shared_no_drop_with::<_, _, PANIC>(len, init)
        }
    }

    /// `fill_iter`-init dispatch for shared-flavor slices.
    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "must inline so the const PANIC flag propagates into the inner fast paths"
    )]
    pub(super) fn try_alloc_slice_shared_fill_iter_inner<T, I, const PANIC: bool>(&self, iter: I) -> Result<NonNull<[T]>, AllocError>
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
    {
        let mut iter = iter.into_iter();
        let len = iter.len();
        let init = |_: usize, dst: &mut MaybeUninit<T>| {
            dst.write(
                iter.next()
                    .expect("caller violated ExactSizeIterator contract: iter.len() reported more elements than iter.next() yields"),
            );
        };
        if const { core::mem::needs_drop::<T>() } {
            self.try_alloc_slice_shared_with::<_, _, PANIC>(len, super::drop_fn_for_slice::<T>(), init)
        } else {
            self.try_alloc_slice_shared_no_drop_with::<_, _, PANIC>(len, init)
        }
    }
}
