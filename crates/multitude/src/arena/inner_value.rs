// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Crate-private scalar allocation helpers shared by the arena APIs.
//!
//! This module holds the `try_alloc_inner_*` family plus the cold slow
//! and oversized paths.

use core::alloc::Layout;
use core::ptr::NonNull;

use allocator_api2::alloc::{AllocError, Allocator};

use super::{
    AllocFlavor, Arena, OversizedLocalGuard, OversizedSharedGuard, ProtectiveHold, SharedArcsIssuedHold, align_offset,
    bump_local_drop_count, bump_shared_drop_count, bumped_exceeds_chunk, current_chunk_evicted, expect_alloc, panic_alloc,
    size_exceeds_normal_alloc, slow_refill_needed, try_bump_fit, write_through_ptr,
};
use crate::internal::constants::{MAX_CHUNK_BYTES, MAX_SMART_PTR_ALIGN};
use crate::internal::drop_list::{DropEntry as InnerDropEntry, drop_shim_one, noop_drop_shim};
use crate::internal::local_chunk::LocalChunk;
use crate::internal::shared_chunk::SharedChunk;
use crate::internal::sync::Ordering;

impl<A: Allocator + Clone> Arena<A> {
    /// Bump-allocate a `T` in the current shared chunk and account for
    /// one outstanding `Arc<T, A>`.
    #[inline]
    #[cfg_attr(test, mutants::skip)] // Hot-path arithmetic mutations are absorbed by chunk-class rounding in `refill_shared`.
    pub(super) fn try_alloc_inner_arc_with<T, F: FnOnce() -> T>(&self, f: F) -> Result<NonNull<T>, AllocError> {
        self.impl_alloc_inner_arc_with::<T, F, false>(f)
    }

    /// Shared fast path for the Arc value-init helpers.
    /// `PANIC=true` panics on allocation failure; `PANIC=false`
    /// propagates `Err`.
    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "Arc fast-path body must inline into every public alloc_arc*/try_alloc_arc* call site so the PANIC const folds"
    )]
    fn impl_alloc_inner_arc_with<T, F: FnOnce() -> T, const PANIC: bool>(&self, f: F) -> Result<NonNull<T>, AllocError> {
        let layout = Layout::new::<T>();
        if layout.align() >= MAX_SMART_PTR_ALIGN {
            if PANIC {
                panic_alloc();
            }
            return Err(AllocError);
        }

        let entry_size = if const { core::mem::needs_drop::<T>() } {
            core::mem::size_of::<InnerDropEntry>()
        } else {
            0
        };

        let bumped = layout.size().max(1);
        if bumped_exceeds_chunk(bumped) {
            let r = self.try_alloc_inner_arc_oversized_with::<T, F>(f);
            return if PANIC { Ok(expect_alloc(r)) } else { r };
        }
        // SAFETY: guarded above. Hint lets the saturating arithmetic
        // in `try_bump_fit` collapse to plain `add`/`sub` after inlining.
        unsafe { core::hint::assert_unchecked(bumped <= MAX_CHUNK_BYTES) };

        loop {
            let data_ptr = self.current_shared.data_ptr.get();
            let drop_back_ptr = self.current_shared.drop_back.get();

            let __fit = try_bump_fit(data_ptr, drop_back_ptr, layout.align().max(1), bumped, entry_size);
            if __fit.fits {
                let aligned_ptr = __fit.aligned_ptr;
                let end_ptr = __fit.end_ptr;
                let new_drop_back_ptr = __fit.new_drop_back_ptr;
                {
                    // `aligned_addr` and `end_addr` lie within `[payload_base, payload_end)`
                    // and `aligned_addr` is naturally aligned for `T`. In stub
                    // state the bump check fails, so we never reach this branch.
                    let value_ptr: *mut T = aligned_ptr.cast::<T>().as_ptr();
                    // SAFETY: passing the bump-fit gate above implies a non-stub slot,
                    // which implies `current_shared.chunk` is `Some`.
                    let chunk = unsafe { self.current_shared.chunk.get().unwrap_unchecked() };

                    // Account before `f` so reentrant refill preserves this +1.
                    self.current_shared.bump_smart_pointers_issued();
                    let hold = SharedArcsIssuedHold { arena: self, chunk };

                    self.current_shared.data_ptr.set(end_ptr);
                    let value_offset = if entry_size > 0 {
                        // Compute is hoisted inside the drop-entry branch so
                        // `T: !Drop` allocations skip the panic surface of the
                        // u16 cast — mirrors `try_alloc_inner_with`.
                        // SAFETY: refcount-positive — chunk held at LARGE
                        // inflation while installed as `current_shared`.
                        let payload_base_addr = unsafe { SharedChunk::<A>::data_ptr(chunk) }.as_ptr() as usize;
                        let raw_value_offset = (aligned_ptr.as_ptr() as usize) - payload_base_addr;
                        // Same bound chain as `impl_alloc_inner_value` (see
                        // there for the full derivation): bump-fit success
                        // implies `raw_value_offset < shared_max_bump_extent::<A>()
                        // = CHUNK_ALIGN − shared_header_size::<A>() ≤ u16::MAX`.
                        debug_assert!(
                            u16::try_from(raw_value_offset).is_ok(),
                            "value_offset must fit in u16; reachable only if oversized chunk leaks into `current_shared`"
                        );
                        // SAFETY: bounded by current-chunk bump extent.
                        unsafe { core::hint::assert_unchecked(u16::try_from(raw_value_offset).is_ok()) };
                        // SAFETY: precondition asserted above.
                        let value_offset = unsafe { u16::try_from(raw_value_offset).unwrap_unchecked() };
                        self.current_shared.drop_back.set(new_drop_back_ptr);
                        let entry_ptr: *mut InnerDropEntry = new_drop_back_ptr.cast::<InnerDropEntry>().as_ptr();
                        let noop_entry = InnerDropEntry::new(noop_drop_shim, value_offset, 1);
                        // SAFETY: `entry_ptr` is valid for one entry.
                        unsafe { core::ptr::write(entry_ptr, noop_entry) };
                        // SAFETY: refcount-positive — chunk is live.
                        unsafe { bump_shared_drop_count(chunk) };
                        value_offset
                    } else {
                        0
                    };

                    // SAFETY: payload-extent invariant. The helper isolates
                    // any T-aligned stack slot to its own frame so this
                    // function's frame stays alignment-bounded.
                    unsafe { write_through_ptr::<T, F>(value_ptr, f) };
                    core::mem::forget(hold);

                    if entry_size > 0 {
                        // `f` succeeded: overwrite the noop drop shim with the
                        // real one. `value_offset` and `len` (both u16) were
                        let entry_ptr: *mut InnerDropEntry = new_drop_back_ptr.cast::<InnerDropEntry>().as_ptr();
                        // SAFETY: `entry_ptr` references the
                        // pre-written noop entry. The `Arc<T>` for
                        // this allocation has not been returned to
                        // the caller yet, so no thread can observe
                        // this slot's `drop_fn` between the
                        // `bump_shared_drop_count` Release above and
                        // this store. Relaxed is sufficient: the
                        // eventual `Arc::drop`'s Release on
                        // `refcount` (which happens-after this store
                        // by program order) carries the new
                        // `drop_fn` to any subsequent `replay_drops`
                        // reader via the standard release-sequence.
                        unsafe { (*entry_ptr).store_drop_fn(drop_shim_one::<T>, Ordering::Relaxed) };
                        let _ = value_offset;
                    }

                    self.charge_alloc_stats(layout.size());
                    // SAFETY: `value_ptr` is non-null and now
                    // points at an initialized `T`.
                    return Ok(unsafe { NonNull::new_unchecked(value_ptr) });
                }
            }

            // If no normal chunk can satisfy the request, use the
            // one-shot oversized path; otherwise refill and retry.
            if size_exceeds_normal_alloc(layout.size(), self.provider.max_normal_alloc) {
                let r = self.try_alloc_inner_arc_oversized_with::<T, F>(f);
                return if PANIC { Ok(expect_alloc(r)) } else { r };
            }
            let needed = slow_refill_needed(layout, entry_size);
            let r = self.refill_shared(needed);
            if PANIC {
                expect_alloc(r);
            } else {
                r?;
            }
        }
    }

    /// Panicking sibling of [`Self::try_alloc_inner_arc_with`].
    ///
    /// See [`Self::alloc_inner_value_or_panic`] for the design rationale.
    #[expect(
        clippy::inline_always,
        reason = "Arc fast-path body must inline into every public panicking alloc_arc/alloc_arc_with call site"
    )]
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // Mirrors try_alloc_inner_arc_with; remaining mutations are defense-in-depth guards and sizing arithmetic absorbed by chunk-class rounding.
    pub(super) fn alloc_inner_arc_with_or_panic<T, F: FnOnce() -> T>(&self, f: F) -> NonNull<T> {
        expect_alloc(self.impl_alloc_inner_arc_with::<T, F, true>(f))
    }

    /// Forwarder to [`LocalChunk::dec_ref`] used by arena-owned callers.
    ///
    /// # Safety
    ///
    /// The caller must own a `+1` on `chunk`.
    #[expect(
        clippy::unused_self,
        reason = "API consistency: same shape as `release_local_chunk`'s sibling helpers; lets callers stay on `&Arena<A>` rather than reach into `LocalChunk`"
    )]
    pub(super) unsafe fn release_local_chunk(&self, chunk: NonNull<LocalChunk<A>>) {
        // SAFETY: caller's invariant — caller owns a +1; `dec_ref` routes
        // the chunk back to its provider (or self-frees) on the zero
        // transition.
        unsafe { LocalChunk::dec_ref(chunk) };
    }

    /// Service a one-shot local allocation above
    /// [`max_normal_alloc`](crate::ArenaBuilder::max_normal_alloc)
    /// without installing the chunk as `current_local`.
    ///
    /// This keeps `current_local` limited to `u16`-addressable chunks.
    #[cold]
    #[inline(never)]
    pub(super) fn try_alloc_inner_oversized_with<T, F: FnOnce() -> T>(&self, f: F, flavor: AllocFlavor) -> Result<NonNull<T>, AllocError> {
        let layout = Layout::new::<T>();
        debug_assert!(layout.align() < MAX_SMART_PTR_ALIGN);

        // Always reserve an entry for `T: needs_drop`, regardless of flavor: the Box
        // flavor still requires a slot so `Box::into_rc` can retarget it from a noop
        // to the real drop shim. The fast-path siblings (`impl_alloc_inner_value` /
        // `impl_alloc_inner_with`) follow the same rule.
        let entry_size = if const { core::mem::needs_drop::<T>() } {
            core::mem::size_of::<InnerDropEntry>()
        } else {
            0
        };

        // `Layout::array`/`Layout::from_size_align` bound `layout.size() <= isize::MAX`;
        // the caller's alignment cap bounds `layout.align()`; `entry_size` is a small constant.
        let needed = slow_refill_needed(layout, entry_size);
        let chunk = self.provider.acquire_local(needed)?;
        // Chunk arrives with refcount inflated to LARGE.
        // SAFETY: refcount-positive — LARGE inflation keeps the chunk live.
        let chunk_ref = unsafe { chunk.as_ref() };
        // SAFETY: refcount-positive — chunk held at LARGE inflation.
        let data_ptr = unsafe { LocalChunk::<A>::data_ptr(chunk) };
        let cap = chunk_ref.capacity;
        let data_addr = data_ptr.as_ptr() as usize;
        // SAFETY: provider post-condition guarantees the chunk fits the request after
        // alignment and drop-entry slack, so neither computation below can fail.
        let aligned = unsafe { align_offset(data_addr, layout.align()).unwrap_unchecked() };
        // SAFETY: same post-condition as above; computation fits well below `usize::MAX`.
        let end = unsafe { aligned.checked_add(layout.size()).unwrap_unchecked() };
        // SAFETY: same post-condition as above.
        unsafe { core::hint::assert_unchecked(end <= cap.saturating_sub(entry_size)) };
        // SAFETY: payload-extent invariant — `aligned` is `T`-aligned and within `[0, cap)`.
        let value_ptr = unsafe { data_ptr.as_ptr().add(aligned).cast::<T>() };

        let guard = OversizedLocalGuard { chunk };

        // SAFETY: payload-extent invariant; `value_ptr` is `T`-aligned and uninitialized.
        unsafe { write_through_ptr::<T, F>(value_ptr, f) };
        core::mem::forget(guard);

        if entry_size > 0 {
            let new_drop_back = cap - entry_size;
            #[expect(
                clippy::cast_ptr_alignment,
                reason = "chunk payloads are 64 KiB aligned (CHUNK_ALIGN), so any `InnerDropEntry` slot computed as `data + new_drop_back` is naturally aligned for `InnerDropEntry`"
            )]
            // SAFETY: payload-extent invariant — back-stack slot lies within the chunk's payload.
            let entry_ptr = unsafe { data_ptr.as_ptr().add(new_drop_back).cast::<InnerDropEntry>() };
            // Box flavor installs a noop here; `Box::drop` runs `drop_in_place` directly and
            // `Box::into_rc` retargets this slot to `drop_shim_one::<T>` on conversion.
            let installed_drop_fn = if matches!(flavor, AllocFlavor::Box) {
                noop_drop_shim
            } else {
                drop_shim_one::<T>
            };
            let entry = InnerDropEntry::new(
                installed_drop_fn,
                u16::try_from(aligned)
                    .expect("oversized chunk payload starts at offset 0; aligned < align < MAX_SMART_PTR_ALIGN ≤ u16::MAX"),
                1,
            );
            // SAFETY: payload-extent invariant.
            unsafe { core::ptr::write(entry_ptr, entry) };
            chunk_ref.drop_count.set(1);
        }

        self.charge_alloc_stats(layout.size());

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

        // SAFETY: `value_ptr` is non-null and now points at an initialized `T`.
        Ok(unsafe { NonNull::new_unchecked(value_ptr) })
    }

    /// Specialized fast path for value-by-value scalar allocation in
    /// the local-flavor chunk.
    ///
    /// Mirrors [`Self::try_alloc_inner_with`] but takes the value
    /// directly instead of a closure. Without a closure there is no
    /// reentrancy hazard and no panic during the value write
    /// (`ptr::write` is infallible), so this path skips the
    /// [`ProtectiveHold`] panic guard, the noop-entry pre-write, and
    /// the post-write eviction recheck. The result is a tighter hot
    /// path: one bump check, one value write, one cursor advance,
    /// optionally one drop-entry write + drop-count bump.
    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "value-fast-path body must inline into every public alloc/alloc_rc/alloc_box call site"
    )]
    // The boundary mutation `bumped > MAX_CHUNK_BYTES` → `>=` on the early
    // routing check is observationally equivalent: `max_normal_alloc` is
    // capped by the builder to `min(MAX_CHUNK_BYTES, max_bump_extent::<A>())`
    // which is strictly less than `MAX_CHUNK_BYTES` (the chunk header eats
    // a few bytes), so a request whose `bumped` equals `MAX_CHUNK_BYTES`
    // necessarily exceeds `max_normal_alloc` and is routed to
    // `try_alloc_inner_oversized_value` regardless of which path got it to
    // `try_alloc_inner_slow_value`. The fast-bump-fit at the same `bumped`
    // would always miss because a max-class chunk's payload is strictly
    // less than `MAX_CHUNK_BYTES`. Other mutations in this function are
    // killed by the slice/value tests under `tests/`.
    #[cfg_attr(test, mutants::skip)]
    pub(super) fn try_alloc_inner_value<T>(&self, value: T, flavor: AllocFlavor) -> Result<NonNull<T>, AllocError> {
        self.impl_alloc_inner_value::<T, false>(value, flavor)
    }

    /// Single source of truth for the value-allocation fast path.
    /// `PANIC=true` makes the inner panic on chunk-allocation failure
    /// (via `panic_alloc()`); `PANIC=false` returns `Err`. With
    /// `PANIC=true`, the const-folded panic branches make `Err` an
    /// unreachable return value, so the panicking wrapper's
    /// `expect_alloc` unwrap collapses to a noop.
    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "value-fast-path body must inline into every public alloc/try_alloc call site so the PANIC const folds"
    )]
    fn impl_alloc_inner_value<T, const PANIC: bool>(&self, value: T, flavor: AllocFlavor) -> Result<NonNull<T>, AllocError> {
        let layout = Layout::new::<T>();
        if layout.align() >= MAX_SMART_PTR_ALIGN {
            if PANIC {
                panic_alloc();
            }
            return Err(AllocError);
        }

        // Reserve a trailing drop entry for any `T: needs_drop` allocation
        // regardless of flavor. For `Box`-flavor allocations the entry is
        // installed with [`noop_drop_shim`] at alloc time, retargeted to
        // [`noop_drop_shim`] (no-op) by `Box::drop` just before
        // `drop_in_place`, and retargeted to `drop_shim_one::<T>` by
        // `Box::into_rc` when the value's lifetime is transferred to a
        // chunk-teardown owner instead.
        //
        // Without an eagerly-reserved entry, `Box::into_rc` would have
        // to install one post-hoc, but installation cannot update the
        // arena's `current_local.drop_back` mirror (the conversion has
        // no arena reference). A subsequent allocation through the
        // arena would then read a stale `drop_back` and collide with
        // the just-installed entry.
        let entry_size = if const { core::mem::needs_drop::<T>() } {
            core::mem::size_of::<InnerDropEntry>()
        } else {
            0
        };

        let data_ptr = self.current_local.data_ptr.get();
        let drop_back_ptr = self.current_local.drop_back.get();
        let bumped = layout.size().max(1);

        if bumped_exceeds_chunk(bumped) {
            let r = self.try_alloc_inner_slow_value::<T>(value, flavor, layout, entry_size);
            return if PANIC { Ok(expect_alloc(r)) } else { r };
        }
        // SAFETY: gated by the explicit `bumped > MAX_CHUNK_BYTES`
        // check immediately above. The hint lets `try_bump_fit`'s
        // saturating arithmetic collapse to plain `add`/`sub` after
        // inlining.
        unsafe { core::hint::assert_unchecked(bumped <= MAX_CHUNK_BYTES) };

        let __fit = try_bump_fit(data_ptr, drop_back_ptr, layout.align().max(1), bumped, entry_size);
        if !__fit.fits {
            let r = self.try_alloc_inner_slow_value::<T>(value, flavor, layout, entry_size);
            return if PANIC { Ok(expect_alloc(r)) } else { r };
        }
        let aligned_ptr = __fit.aligned_ptr;
        let end_ptr = __fit.end_ptr;
        let new_drop_back_ptr = __fit.new_drop_back_ptr;

        // SAFETY: chunk-present invariant — see `try_alloc_inner_with`.
        // Only loaded when actually used (entry_size > 0 path). For
        // `T: !Drop` non-Box flavors `entry_size == 0` and the chunk
        // pointer is never needed in this function — the load is
        // deferred and elided by LLVM.
        let chunk_lazy = || unsafe { self.current_local.chunk.get().unwrap_unchecked() };

        match flavor {
            AllocFlavor::SimpleRef => {
                self.current_local_pinned.set(true);
            }
            AllocFlavor::Rc | AllocFlavor::Box => {
                self.current_local.bump_smart_pointers_issued();
            }
        }

        let value_ptr: *mut T = aligned_ptr.cast::<T>().as_ptr();
        // Publish the new bump cursor BEFORE `ptr::write` of the value.
        // For large `T`, the value write is many vmovups stores; the
        // cursor store sits behind them in the store buffer and stalls
        // the next iter's `data_ptr.get()` store-forwarding load.
        // Publishing first lets the next iter's load complete in
        // parallel with the value fill (see `try_alloc_slice_local_copy`
        // for the detailed rationale).
        self.current_local.data_ptr.set(end_ptr);
        // SAFETY: payload-extent invariant; `aligned_addr` is `T`-aligned
        // and lies within `[payload_base, payload_end)`. `ptr::write`
        // is infallible — no panic surface, no `ProtectiveHold` needed.
        unsafe { core::ptr::write(value_ptr, value) };

        if entry_size > 0 {
            let chunk = chunk_lazy();
            // SAFETY: refcount-positive — chunk held at LARGE inflation.
            let payload_base_addr = unsafe { LocalChunk::<A>::data_ptr(chunk) }.as_ptr() as usize;
            let raw_value_offset = (aligned_ptr.as_ptr() as usize) - payload_base_addr;
            // Bound chain:
            //   raw_value_offset < bump_extent
            //                    = capacity.min(local_max_bump_extent::<A>())
            //                    ≤ local_max_bump_extent::<A>()
            //                    = CHUNK_ALIGN − local_header_size::<A>()    (header ≥ 1)
            //                    ≤ 65536 − 1 = 65535 = u16::MAX
            // i.e. successful bump-fit places the allocation strictly
            // inside the first 64 KiB tile of the chunk's payload, so
            // the offset fits in `u16`. (Note: `MAX_CHUNK_BYTES = 65536`
            // is *not* itself ≤ `u16::MAX` — the real bound is
            // `local_max_bump_extent::<A>()`, which is strictly less.)
            debug_assert!(
                u16::try_from(raw_value_offset).is_ok(),
                "value_offset must fit in u16; reachable only if oversized chunk leaks into `current_local`"
            );
            // SAFETY: bounded by the chain above.
            unsafe { core::hint::assert_unchecked(u16::try_from(raw_value_offset).is_ok()) };
            // SAFETY: precondition asserted above; this conversion has no panic surface.
            let value_offset = unsafe { u16::try_from(raw_value_offset).unwrap_unchecked() };
            self.current_local.drop_back.set(new_drop_back_ptr);
            let entry_ptr: *mut InnerDropEntry = new_drop_back_ptr.cast::<InnerDropEntry>().as_ptr();
            // `Box` installs a noop shim (retargeted at `Box::into_rc` time);
            // `Rc`/`SimpleRef` install the real shim now.
            let shim: unsafe fn(*mut u8, usize) = if matches!(flavor, AllocFlavor::Box) {
                noop_drop_shim
            } else {
                drop_shim_one::<T>
            };
            let entry = InnerDropEntry::new(shim, value_offset, 1);
            // SAFETY: `entry_ptr` is valid for one entry.
            unsafe { core::ptr::write(entry_ptr, entry) };
            // SAFETY: refcount-positive — per-flavor `+1` (smart-pointer
            // hold or `ProtectiveHold`) keeps `chunk` alive across this
            // call.
            unsafe { bump_local_drop_count(chunk) };
        }

        self.charge_alloc_stats(layout.size());

        // SAFETY: `value_ptr` is non-null and points at an initialized `T`.
        Ok(unsafe { NonNull::new_unchecked(value_ptr) })
    }

    /// Panicking sibling of [`Self::try_alloc_inner_value`].
    ///
    /// Returns `NonNull<T>` directly. On allocation failure this calls
    /// [`panic_alloc`] instead of returning `Err`. Public panicking
    /// entry points (`alloc`, `alloc_box`, `alloc_rc`) call this
    /// sibling so the inlined hot path doesn't carry a dead
    /// `Result<NonNull<T>, AllocError>` niche check (the const-folded
    /// `PANIC=true` instantiation of `impl_alloc_inner_value` panics
    /// on every error site, so `expect_alloc` reduces to an Ok-unwrap).
    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "value-fast-path body must inline into every public panicking alloc/alloc_rc/alloc_box call site"
    )]
    #[cfg_attr(test, mutants::skip)] // Mirrors try_alloc_inner_value; remaining mutations are defense-in-depth guards absorbed by chunk-class rounding.
    pub(super) fn alloc_inner_value_or_panic<T>(&self, value: T, flavor: AllocFlavor) -> NonNull<T> {
        expect_alloc(self.impl_alloc_inner_value::<T, true>(value, flavor))
    }

    /// Cold tail of [`Self::try_alloc_inner_value`] handling oversized
    /// routing and the refill/retry loop. Reached only when the
    /// fast-path bump check fails.
    #[cold]
    #[cfg_attr(test, mutants::skip)] // Cold routing & needed-arithmetic mutations absorbed by chunk-class rounding.
    pub(super) fn try_alloc_inner_slow_value<T>(
        &self,
        value: T,
        flavor: AllocFlavor,
        layout: Layout,
        entry_size: usize,
    ) -> Result<NonNull<T>, AllocError> {
        if size_exceeds_normal_alloc(layout.size(), self.provider.max_normal_alloc) {
            return self.try_alloc_inner_oversized_value::<T>(value, flavor);
        }

        let needed = slow_refill_needed(layout, entry_size);
        self.refill_local(needed)?;
        // `refill_local` post-condition: the refreshed chunk has at least
        // `needed` bytes, which already accounts for alignment slack and the
        // drop-entry slot, so the bump fit cannot fail.
        self.try_alloc_inner_value::<T>(value, flavor)
    }

    /// Oversized one-shot value allocation. Mirror of
    /// [`Self::try_alloc_inner_oversized_with`] for value-by-value
    /// callers; skips the panic-recovery [`OversizedLocalGuard`]
    /// because `ptr::write` cannot panic.
    #[cold]
    #[cfg_attr(test, mutants::skip)] // Cold oversized path; drop-entry-guard mutations are equivalent for non-drop T and Box-flavor.
    pub(super) fn try_alloc_inner_oversized_value<T>(&self, value: T, flavor: AllocFlavor) -> Result<NonNull<T>, AllocError> {
        let layout = Layout::new::<T>();
        debug_assert!(layout.align() < MAX_SMART_PTR_ALIGN);

        // See [`Self::try_alloc_inner_oversized_with`] for the Box-flavor rationale.
        let entry_size = if const { core::mem::needs_drop::<T>() } {
            core::mem::size_of::<InnerDropEntry>()
        } else {
            0
        };

        // Bounded by Layout invariants and caller's alignment cap; see siblings.
        let needed = slow_refill_needed(layout, entry_size);
        let chunk = self.provider.acquire_local(needed)?;
        // chunk arrives with refcount inflated to LARGE.
        // SAFETY: refcount-positive — LARGE inflation keeps the chunk live.
        let chunk_ref = unsafe { chunk.as_ref() };
        // SAFETY: refcount-positive — chunk held at LARGE inflation.
        let data_ptr = unsafe { LocalChunk::<A>::data_ptr(chunk) };
        let cap = chunk_ref.capacity;
        let data_addr = data_ptr.as_ptr() as usize;
        // SAFETY: provider post-condition guarantees the chunk fits the request.
        let aligned = unsafe { align_offset(data_addr, layout.align()).unwrap_unchecked() };
        // SAFETY: same post-condition as above; computation fits well below `usize::MAX`.
        let end = unsafe { aligned.checked_add(layout.size()).unwrap_unchecked() };
        // SAFETY: provider post-condition.
        unsafe { core::hint::assert_unchecked(end <= cap.saturating_sub(entry_size)) };
        // SAFETY: payload-extent invariant — `aligned` is `T`-aligned and within `[0, cap)`.
        let value_ptr = unsafe { data_ptr.as_ptr().add(aligned).cast::<T>() };

        // SAFETY: payload-extent invariant; `value_ptr` is `T`-aligned
        // and uninitialized. `ptr::write` is infallible.
        unsafe { core::ptr::write(value_ptr, value) };

        if entry_size > 0 {
            let new_drop_back = cap - entry_size;
            #[expect(
                clippy::cast_ptr_alignment,
                reason = "chunk payloads are 64 KiB aligned (CHUNK_ALIGN), so any `InnerDropEntry` slot computed as `data + new_drop_back` is naturally aligned for `InnerDropEntry`"
            )]
            // SAFETY: payload-extent invariant — back-stack slot lies within the chunk's payload.
            let entry_ptr = unsafe { data_ptr.as_ptr().add(new_drop_back).cast::<InnerDropEntry>() };
            // See [`Self::try_alloc_inner_oversized_with`] for the Box-flavor rationale.
            let installed_drop_fn = if matches!(flavor, AllocFlavor::Box) {
                noop_drop_shim
            } else {
                drop_shim_one::<T>
            };
            let entry = InnerDropEntry::new(
                installed_drop_fn,
                u16::try_from(aligned)
                    .expect("oversized chunk payload starts at offset 0; aligned < align < MAX_SMART_PTR_ALIGN ≤ u16::MAX"),
                1,
            );
            // SAFETY: payload-extent invariant.
            unsafe { core::ptr::write(entry_ptr, entry) };
            chunk_ref.drop_count.set(1);
        }

        self.charge_alloc_stats(layout.size());

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

        // SAFETY: `value_ptr` is non-null and now points at an initialized `T`.
        Ok(unsafe { NonNull::new_unchecked(value_ptr) })
    }

    /// Service a one-shot `Arc` allocation that exceeds
    /// [`max_normal_alloc`](crate::ArenaBuilder::max_normal_alloc)
    /// without routing the oversized shared chunk through
    /// `current_shared`. Mirror of
    /// [`Self::try_alloc_inner_oversized_with`] for the shared flavor.
    #[inline(never)]
    pub(super) fn try_alloc_inner_arc_oversized_with<T, F: FnOnce() -> T>(&self, f: F) -> Result<NonNull<T>, AllocError> {
        let layout = Layout::new::<T>();
        debug_assert!(layout.align() < MAX_SMART_PTR_ALIGN);

        let entry_size = if const { core::mem::needs_drop::<T>() } {
            core::mem::size_of::<InnerDropEntry>()
        } else {
            0
        };

        // Bounded by Layout invariants and caller's alignment cap; see siblings.
        let needed = slow_refill_needed(layout, entry_size);
        let chunk = self.provider.acquire_shared(needed)?;
        // SAFETY: refcount-positive — LARGE inflation keeps the chunk live.
        let chunk_ref = unsafe { chunk.as_ref() };
        // SAFETY: refcount-positive — chunk held at LARGE inflation.
        let data_ptr = unsafe { SharedChunk::<A>::data_ptr(chunk) };
        let cap = chunk_ref.capacity;
        let data_addr = data_ptr.as_ptr() as usize;
        // SAFETY: provider post-condition guarantees the chunk fits the request.
        let aligned = unsafe { align_offset(data_addr, layout.align()).unwrap_unchecked() };
        // SAFETY: same post-condition as above; computation fits well below `usize::MAX`.
        let end = unsafe { aligned.checked_add(layout.size()).unwrap_unchecked() };
        // SAFETY: provider post-condition.
        unsafe { core::hint::assert_unchecked(end <= cap.saturating_sub(entry_size)) };
        // SAFETY: payload-extent invariant — `aligned` is `T`-aligned and within `[0, cap)`.
        let value_ptr = unsafe { data_ptr.as_ptr().add(aligned).cast::<T>() };

        let guard = OversizedSharedGuard { chunk };

        // SAFETY: payload-extent invariant; `value_ptr` is `T`-aligned and uninitialized.
        unsafe { write_through_ptr::<T, F>(value_ptr, f) };
        core::mem::forget(guard);

        if entry_size > 0 {
            let new_drop_back = cap - entry_size;
            #[expect(
                clippy::cast_ptr_alignment,
                reason = "chunk payloads are 64 KiB aligned (CHUNK_ALIGN), so any `InnerDropEntry` slot computed as `data + new_drop_back` is naturally aligned for `InnerDropEntry`"
            )]
            // SAFETY: payload-extent invariant.
            let entry_ptr = unsafe { data_ptr.as_ptr().add(new_drop_back).cast::<InnerDropEntry>() };
            let entry = InnerDropEntry::new(
                drop_shim_one::<T>,
                u16::try_from(aligned)
                    .expect("oversized chunk payload starts at offset 0; aligned < align < MAX_SMART_PTR_ALIGN ≤ u16::MAX"),
                1,
            );
            // SAFETY: payload-extent invariant.
            unsafe { core::ptr::write(entry_ptr, entry) };
            // No other thread can yet observe this chunk: the
            // inflation has not been published via any cross-thread handoff.
            chunk_ref.drop_count.store(1, Ordering::Relaxed);
        }

        self.charge_alloc_stats(layout.size());

        // SAFETY: chunk held LARGE while we acted as its sole tenant.
        unsafe { SharedChunk::reconcile_swap_out(chunk, 1) };

        let _ = end;
        // SAFETY: `value_ptr` is non-null and now points at an initialized `T`.
        Ok(unsafe { NonNull::new_unchecked(value_ptr) })
    }

    #[cfg_attr(test, mutants::skip)] // Hot path; equivalent mutations absorbed by chunk-class rounding and the drop-list replay tests.
    pub(super) fn try_alloc_inner_with<T, F: FnOnce() -> T>(&self, f: F, flavor: AllocFlavor) -> Result<NonNull<T>, AllocError> {
        self.impl_alloc_inner_with::<T, F, false>(f, flavor)
    }

    /// Single source of truth for the `_with`-style value-allocation
    /// fast path. `PANIC=true` panics on chunk-allocation failure;
    /// `PANIC=false` propagates `Err`. The const folds at
    /// monomorphization so each instantiation produces the same
    /// machine code as a hand-written try/panic pair would.
    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "value-fast-path body must inline into every public alloc_with/try_alloc_with call site so the PANIC const folds"
    )]
    fn impl_alloc_inner_with<T, F: FnOnce() -> T, const PANIC: bool>(&self, f: F, flavor: AllocFlavor) -> Result<NonNull<T>, AllocError> {
        let layout = Layout::new::<T>();
        if layout.align() >= MAX_SMART_PTR_ALIGN {
            if PANIC {
                panic_alloc();
            }
            return Err(AllocError);
        }

        // Reserve a trailing drop entry for any `T: needs_drop` allocation
        // regardless of flavor. For `Box`-flavor allocations the entry is
        // installed with [`noop_drop_shim`] at alloc time, retargeted to
        // [`noop_drop_shim`] (no-op) by `Box::drop` just before
        // `drop_in_place`, and retargeted to `drop_shim_one::<T>` by
        // `Box::into_rc` when the value's lifetime is transferred to a
        // chunk-teardown owner instead.
        //
        // Without an eagerly-reserved entry, `Box::into_rc` would have
        // to install one post-hoc, but installation cannot update the
        // arena's `current_local.drop_back` mirror (the conversion has
        // no arena reference). A subsequent allocation through the
        // arena would then read a stale `drop_back` and collide with
        // the just-installed entry.
        let entry_size = if const { core::mem::needs_drop::<T>() } {
            core::mem::size_of::<InnerDropEntry>()
        } else {
            0
        };

        let data_ptr = self.current_local.data_ptr.get();
        let drop_back_ptr = self.current_local.drop_back.get();
        let bumped = layout.size().max(1);
        // `.max(1)` ensures the bump check fails in stub state for ZSTs
        // (where `layout.size() == 0` and `aligned_addr == data_ptr_addr`
        // could otherwise route past the chunk-present check). Costs
        // at most one wasted byte per ZST allocation in real chunks.

        if bumped_exceeds_chunk(bumped) {
            let r = self.try_alloc_inner_slow_with::<T, F>(f, flavor, layout, entry_size);
            return if PANIC { Ok(expect_alloc(r)) } else { r };
        }
        // SAFETY: gated by the explicit `bumped > MAX_CHUNK_BYTES`
        // check immediately above.
        unsafe { core::hint::assert_unchecked(bumped <= MAX_CHUNK_BYTES) };

        // Single-branch fit check; see [`try_bump_fit`] for the
        // overflow / alignment / bound semantics. Any miss routes to
        // the cold slow path, which picks between oversized routing
        // and refill+retry.
        let __fit = try_bump_fit(data_ptr, drop_back_ptr, layout.align().max(1), bumped, entry_size);
        if !__fit.fits {
            let r = self.try_alloc_inner_slow_with::<T, F>(f, flavor, layout, entry_size);
            return if PANIC { Ok(expect_alloc(r)) } else { r };
        }
        let aligned_ptr = __fit.aligned_ptr;
        let end_ptr = __fit.end_ptr;
        let new_drop_back_ptr = __fit.new_drop_back_ptr;

        // SAFETY: chunk-present invariant — fast-path gate above
        // implies `aligned + bumped <= drop_back`, with `drop_back !=
        // dangling(1)` (stub state has data_ptr == drop_back == 1, so
        // `bumped > 0` would have failed the gate).
        let chunk = unsafe { self.current_local.chunk.get().unwrap_unchecked() };

        // Protect the reserved chunk across reentrant calls from `f`.
        match flavor {
            AllocFlavor::SimpleRef => {
                self.current_local_pinned.set(true);
            }
            AllocFlavor::Rc | AllocFlavor::Box => {
                self.current_local.bump_smart_pointers_issued();
            }
        }

        // On success, the hold becomes the smart pointer's or pin entry's +1.
        let guard = ProtectiveHold::<A> {
            arena: self,
            chunk,
            flavor,
        };

        // Pre-advance the bump cursor and pre-write a noop drop entry
        // *before* invoking the user closure: a reentrant `alloc_*`
        // call from inside `f` must not see un-advanced bump state
        // and overlap our reservation.
        self.current_local.data_ptr.set(end_ptr);
        // The `payload_base_addr` / `value_offset` compute (and its u16
        // panic surface) live inside the `if entry_size > 0` arm so
        // they vanish for `T: !Drop` non-Box flavors — LLVM otherwise
        // hoists the panic check unconditionally and bloats the loop.
        let value_offset = if entry_size > 0 {
            // SAFETY: refcount-positive — chunk held at LARGE inflation
            // while installed as `current_local`.
            let payload_base_addr = unsafe { LocalChunk::<A>::data_ptr(chunk) }.as_ptr() as usize;
            let raw_value_offset = (aligned_ptr.as_ptr() as usize) - payload_base_addr;
            // Same bound chain as `impl_alloc_inner_value` — successful
            // bump-fit places the offset within `local_max_bump_extent::<A>()
            // = CHUNK_ALIGN − local_header_size::<A>() ≤ u16::MAX`.
            debug_assert!(
                u16::try_from(raw_value_offset).is_ok(),
                "value_offset must fit in u16; reachable only if oversized chunk leaks into `current_local`"
            );
            // SAFETY: bounded by current-chunk bump extent.
            unsafe { core::hint::assert_unchecked(u16::try_from(raw_value_offset).is_ok()) };
            // SAFETY: precondition asserted above.
            let value_offset = unsafe { u16::try_from(raw_value_offset).unwrap_unchecked() };
            self.current_local.drop_back.set(new_drop_back_ptr);
            let entry_ptr: *mut InnerDropEntry = new_drop_back_ptr.cast::<InnerDropEntry>().as_ptr();
            let noop_entry = InnerDropEntry::new(noop_drop_shim, value_offset, 1);
            // SAFETY: `entry_ptr` is valid for one entry.
            unsafe { core::ptr::write(entry_ptr, noop_entry) };
            // SAFETY: refcount-positive — protective hold keeps `chunk` alive.
            unsafe { bump_local_drop_count(chunk) };
            value_offset
        } else {
            0
        };

        let value_ptr: *mut T = aligned_ptr.cast::<T>().as_ptr();
        // SAFETY: payload-extent invariant — `aligned_addr` lies within
        // `[payload_base, payload_end)` and is naturally aligned for `T`.
        unsafe { write_through_ptr::<T, F>(value_ptr, f) };
        core::mem::forget(guard);

        // `f` may have reentrantly replaced `current_local`; `chunk`
        // is still held. The post-closure recheck is only needed when
        // there is a real drop entry to overwrite (`entry_size > 0`).
        // For `T: !Drop` non-Box flavors `entry_size` is a const-folded
        // 0 and the eviction path's only side-effect (`charge_alloc_stats`)
        // is the same as the in-place success path; skipping the recheck
        // saves a chunk pointer reload, an 8-byte comparison, and a
        // never-taken branch on every hot iteration.
        //
        // We compare via raw `*mut LocalChunk` so LLVM emits a single
        // 8-byte equality test instead of treating the `Option<NonNull<_>>`
        // as a discriminant+payload pair.
        if entry_size > 0 {
            let cur_chunk_addr = self.current_local.chunk.get().map_or(0_usize, |c| c.as_ptr().cast::<u8>() as usize);
            if current_chunk_evicted(cur_chunk_addr, chunk.as_ptr().cast::<u8>() as usize) {
                // Cold: closure caused chunk eviction during the call.
                // SAFETY: `chunk` was the active local chunk and still
                // holds the LARGE inflation; the value at `value_ptr` is
                // initialized; back-stack and bump-cursor were
                // pre-advanced before the closure ran.
                let ptr = unsafe { self.commit_alloc_after_eviction::<T>(flavor, new_drop_back_ptr, entry_size, layout.size(), value_ptr) };
                return Ok(ptr);
            }
            // For `Rc`/`SimpleRef` flavors, retarget the pre-written
            // noop drop shim to the real one. For `Box` flavor we
            // leave the entry as a noop because `Box::drop` runs
            // `drop_in_place` directly; `Box::into_rc` retargets the
            // entry to the real shim at conversion time.
            //
            // `value_offset` and `len` (both u16) were already written
            // by the pre-closure noop entry and are unchanged, so we
            // only need to update the 8-byte `drop_fn` pointer at
            // offset 0 of the `InnerDropEntry`.
            if !matches!(flavor, AllocFlavor::Box) {
                let entry_ptr: *mut InnerDropEntry = new_drop_back_ptr.cast::<InnerDropEntry>().as_ptr();
                // SAFETY: `entry_ptr` references the pre-written
                // noop entry. `LocalChunk: !Send` makes the chunk
                // owner-thread exclusive, so a non-atomic update via
                // `store_drop_fn(Relaxed)` is sound (no cross-thread
                // reader is possible).
                unsafe { (*entry_ptr).store_drop_fn(drop_shim_one::<T>, Ordering::Relaxed) };
            }
            let _ = value_offset;
            // `slot.drop_back` and `chunk.drop_count` were pre-advanced.
        }

        // `slot.data_ptr` was pre-advanced before the closure ran. A
        // reentrant in-chunk alloc may have advanced it further; we
        // intentionally leave that further value in place.

        self.charge_alloc_stats(layout.size());

        // SAFETY: `value_ptr` is non-null and now points at an initialized `T`.
        Ok(unsafe { NonNull::new_unchecked(value_ptr) })
    }

    /// Panicking sibling of [`Self::try_alloc_inner_with`].
    ///
    /// See [`Self::alloc_inner_value_or_panic`] for the design rationale.
    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "value-fast-path body must inline into every public panicking alloc_with/alloc_rc_with/alloc_box_with call site"
    )]
    #[cfg_attr(test, mutants::skip)] // Mirrors try_alloc_inner_with; remaining mutations are defense-in-depth guards and eviction-path equivalence.
    pub(super) fn alloc_inner_with_or_panic<T, F: FnOnce() -> T>(&self, f: F, flavor: AllocFlavor) -> NonNull<T> {
        expect_alloc(self.impl_alloc_inner_with::<T, F, true>(f, flavor))
    }

    #[cold]
    #[cfg_attr(test, mutants::skip)] // Cold slow path; see `try_alloc_inner_slow_value`.
    pub(super) fn try_alloc_inner_slow_with<T, F: FnOnce() -> T>(
        &self,
        f: F,
        flavor: AllocFlavor,
        layout: Layout,
        entry_size: usize,
    ) -> Result<NonNull<T>, AllocError> {
        if size_exceeds_normal_alloc(layout.size(), self.provider.max_normal_alloc) {
            return self.try_alloc_inner_oversized_with::<T, F>(f, flavor);
        }

        let needed = slow_refill_needed(layout, entry_size);
        self.refill_local(needed)?;
        // `refill_local` post-condition: the refreshed chunk has at least
        // `needed` bytes, which already accounts for alignment slack and the
        // drop-entry slot, so the bump fit cannot fail.
        self.try_alloc_inner_with::<T, F>(f, flavor)
    }

    /// Cold path: closure-induced eviction completed the chunk's
    /// transition to the pinned list (`SimpleRef`) or smart-pointer
    /// container while we held the protective +1; commit the drop
    /// entry/refcount on the now-pinned chunk and return without
    /// touching `current_local`.
    ///
    /// # Safety
    /// `chunk` must be live (protective hold transferred from caller),
    /// `aligned_addr`/`new_drop_back_addr` must lie in `chunk`'s
    /// payload, and `value_ptr` must already point at an initialized `T`.
    #[cold]
    #[inline(never)]
    unsafe fn commit_alloc_after_eviction<T>(
        &self,
        flavor: AllocFlavor,
        new_drop_back_ptr: NonNull<u8>,
        entry_size: usize,
        size: usize,
        value_ptr: *mut T,
    ) -> NonNull<T> {
        // For `Rc`/`SimpleRef` flavors, retarget the pre-written noop
        // drop entry to the real shim. For `Box` flavor we leave the
        // entry as a noop because `Box::drop` runs `drop_in_place`
        // directly; `Box::into_rc` retargets the entry at conversion
        // time.
        //
        // The pre-closure path already wrote a complete `InnerDropEntry`
        // (with the correct `value_offset` and `len = 1`) at
        // `new_drop_back_ptr` and pre-incremented `chunk.drop_count`.
        // We only need to overwrite the 8-byte `drop_fn` pointer at
        // offset 0; recomputing `value_offset` here would add an
        // otherwise-unnecessary `u16::try_from` panic surface to the
        // cold path.
        if entry_size > 0 && !matches!(flavor, AllocFlavor::Box) {
            let entry_ptr: *mut InnerDropEntry = new_drop_back_ptr.cast::<InnerDropEntry>().as_ptr();
            // SAFETY: `entry_ptr` references the pre-written noop
            // entry. Local chunks are owner-thread exclusive
            // (`LocalChunk: !Send`); Relaxed is sufficient.
            unsafe { (*entry_ptr).store_drop_fn(drop_shim_one::<T>, Ordering::Relaxed) };
        }

        self.charge_alloc_stats(size);

        // SAFETY: `value_ptr` is non-null and now points at an initialized `T`.
        unsafe { NonNull::new_unchecked(value_ptr) }
    }
}
