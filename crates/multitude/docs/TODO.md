# TODO

- Eliminate vec of chunk mutators. This should be a linked list

- When expanding a vec that's in an oversized chunk, we should return the
previous chunk back to the system allocator instead of holding onto it in the
retired chunk list.

- **Remove the `capacity` field from `LocalChunk` / `SharedChunk` headers**
  (saves 8 bytes per chunk header). The field is redundant with the
  slice-tail length metadata of the DST `data: [UnsafeCell<u8>]`, except on
  the cache-pop path: chunks sit on the provider freelist linked via a
  thin `*mut u8` and `header_to_fat` (`src/internal/local_chunk.rs:185`,
  matching code in `src/internal/shared_chunk.rs`) currently reads the
  stored `capacity` to reconstruct the slice length. The provider already
  pins each freelist to a single class (`local_cache_class` /
  `shared_cache_class` in `src/internal/chunk_provider.rs`), so the size is
  known by context. Thread `SizeClass` through the pop path, reconstruct
  `cap = class.bytes() - header_size()` in `header_to_fat`, do the same
  for `SharedChunk`, and update `destroy` callers (which need the layout)
  to take the class too. Verify with the gungraun benchmarks that the
  header shrink doesn't move alignment / payload-offset and that the
  size-class plumbing is not on a hot path.

- **Batched shared-chunk refcount increments on Arc/Box allocation.** Each
  `Arc`/`Box` allocation currently performs an atomic `fetch_add(1, Relaxed)`
  on the owning `SharedChunk`'s `ref_count` (see `acquire_shared_chunk_ref` in
  `src/arena/alloc_value.rs` calling `SharedChunk::inc_ref` in
  `src/internal/shared_chunk.rs`). The previous generation of the crate
  avoided this by accumulating refcount increments locally in the active
  shared mutator and flushing them to the atomic counter only at
  chunk-transition / mutator-drop time. Bring this back: add a
  `pending_refs: usize` counter to the shared `ChunkMutator`; have
  `acquire_shared_chunk_ref` bump it instead of calling `inc_ref` per
  allocation; flush via a single `fetch_add(pending_refs, Relaxed)` when the
  mutator is uninstalled or dropped. Teardown / `dec_ref` must observe that
  the chunk's effective refcount is `atomic_ref_count + pending_refs` while
  the mutator is installed, and the overflow guard must still trip when the
  combined value would saturate.

## Unsafe-block-reduction opportunities (analysis 2026-06-06)

### Medium confidence (sound, but verify ordering/lifetimes)

- **Hoist `Arc::from_raw` / `Box::from_raw` out of the alloc retry loops.** The
  shared-slice and uninit retry loops construct the smart pointer separately in
  each exit arm (current-chunk fast path, oversized closure, post-refill):
  `src/arena/alloc_slice_arc.rs:170,183,217,228,253`,
  `src/arena/alloc_value.rs:603,625,662,675` (uninit arc/slice-arc), and
  `src/arena/alloc_unsized.rs:237,251` (dst box). Refactor each loop to `break`
  with the raw `NonNull` payload pointer and perform a single
  `Arc::from_raw`/`Box::from_raw` after the loop. CAVEAT: the `ChunkRef::forget()`
  that retains the fresh `+1` refcount, and any `publish_drop_count()` /
  stats recording, must still happen *before* the `break` in every arm so the
  adopted reference survives to the single construction point.
  Estimated: **net -6 blocks** across the three files.

- **Centralize the "initialized `NonNull` -> `&'a mut`" reborrows in
  `uninit.rs`.** `src/internal/uninit.rs:89,146,200,249,339,450` repeat
  `unsafe { ptr.as_mut() }` / `unsafe { &mut *ptr.as_ptr() }` at the end of the
  init paths. Add one private helper `fn initialized_mut<'a, T: ?Sized>(ptr:
  NonNull<T>) -> &'a mut T` that holds the single `unsafe`. CAVEAT: the helper
  forges the `'a` lifetime, so it must stay private to `uninit.rs` and only be
  called after the ticket has been consumed and the value fully initialized
  (same precondition the call sites already satisfy).
  Estimated: **net -5 blocks**.

- **Encapsulate the `DropEntry::placeholder` raw writes in `chunk_mutator.rs`.**
  `src/internal/chunk_mutator.rs:352-354,376-378,424-426,517-519` all do
  `unsafe { core::ptr::write(drop_slot.as_ptr(), DropEntry::placeholder(...)) }`.
  Add a private `fn write_drop_placeholder(drop_slot, value_offset, len)`
  holding the single `unsafe`. CAVEAT: this is a *safe* fn wrapping an unchecked
  write, sound only because every caller passes a freshly reserved, aligned,
  exclusively-owned slot from `try_reserve_drop_entry`; keep it private and
  document that invariant on the helper.
  Estimated: **net -3 blocks**.

- **Drop `chunk_ptr_unchecked` (`unwrap_unchecked`) in favor of an early
  `self.chunk?`.** `src/internal/chunk_mutator.rs:155-157` plus call sites `229`,
  `252` rely on the sentinel "empty mutator" proof to justify
  `unsafe { self.chunk.unwrap_unchecked() }`. Take `let chunk = self.chunk?;`
  up front; the subsequent `try_alloc*` already returns `None` for the empty
  mutator, so behavior is preserved without the unchecked unwrap. CAVEAT: confirm
  the `None`-propagation matches the sentinel behavior exactly for the empty
  mutator before removing the helper.
  Estimated: **net -3 blocks**.

### Lower value

- **`PrefixedUtf16Ptr` newtype** wrapping the length-prefixed `NonNull<u16>` in
  `src/strings/arc_utf16_str.rs:66,76` and `src/strings/box_utf16_str.rs:61,70,98`,
  with `len()` / `as_utf16_str()` / `as_mut_utf16_str()` methods holding the
  `read_prefix_len` + `from_raw_parts` + `from_slice_unchecked` unsafe. Sound only
  if constructed exclusively via the existing unsafe `from_raw` paths.
  Estimated: **net -2 blocks**.

### Deferred (perf-risk)

- **Consolidate try-current/oversized/refill loops.** `impl_alloc_local_with`,
  `impl_alloc_smart_with`, the slice-Arc copy/fill loops, prefixed shared
  loop, UTF-16 transcoding loop, and DST Box/Arc smart loops repeat the
  same "try current reservation; route oversized; refill and retry" shape.
  Each is `#[inline(always)]` on the allocation fast path; structural
  differences (local vs shared, with-drop vs without, ZST/uninit/zeroed
  branches, stats recording, slot-init helpers, different smart-pointer
  constructions) make a single macro/closure abstraction either fragile
  (closure-state capture risks codegen drift) or unwieldy (a macro with
  many positional knobs hurts readability without saving meaningful
  unsafe). Deferred to keep gungraun instruction counts stable. See
  simplification-report item 1.2.

## Simplification opportunities (analysis 2026-06-08)

### High-confidence wins (mechanical, no risk)

- **Dedup the "prefixed slice" arithmetic in `chunk_mutator.rs`.**
  `try_alloc_uninit_slice_prefixed` (`src/internal/chunk_mutator.rs:290-320`)
  and `try_alloc_uninit_slice_with_drop_prefixed`
  (`src/internal/chunk_mutator.rs:381-417`) compute the same
  `prefix_size / payload_offset / payload_bytes / total` and run the same
  unsafe block writing the prefix word and projecting the payload
  `NonNull`. Extract `fn try_alloc_prefixed_payload<T>(&self, len: usize)
  -> Option<(InChunk<…>, /*payload_addr*/ usize)>` that owns the layout
  math + prefix write; the two callers add only the drop-entry plumbing.
  Touches one file, ~25 lines.

- **Make `ChunkMutator::from_owned` reuse the payload-range math.**
  `from_owned` (`src/internal/chunk_mutator.rs:~65-85`) re-derives
  `start_addr / aligned_end_addr / aligned_end_offset` from `payload_ptr`
  / `capacity`; `payload_range`
  (`src/internal/chunk_mutator.rs:122-133`) already encapsulates exactly
  that calculation. Add a `payload_range_for(chunk)` taking a
  `NonNull<C>` (or set `self.chunk` first and call `payload_range()`).
  Touches one file, ~10 lines.

### Medium-confidence (worth doing, slight refactor)

- **Unify `release_local` / `release_shared` cache-bypass branches.**
  `src/internal/chunk_provider.rs:466-487` vs `494-505`. Same
  structure: read total → if uncacheable or below floor, destroy +
  release_bytes → else push to cache. Differences: which floor atomic
  (`Acquire` vs `Relaxed`), which `destroy`, single-threaded
  `local_cache.with` vs `push_shared`. Extract
  `fn should_bypass_cache(&self, total: usize, floor: &AtomicU8, ord:
  Ordering) -> bool` for at least the decision. The `Ordering`
  difference is real and intentional; pass it as a parameter.

- **Unify `acquire_normal_local` / `acquire_normal_shared`.**
  `src/internal/chunk_provider.rs:267-292` vs `377-393`. Same
  "advance floor if needed, pop cache, else allocate fresh" shape.
  Extract the floor-bump/pop control flow; pass flavor-specific pop /
  reinit / allocate-fresh closures. Slight closure overhead — verify
  codegen still inlines on the hot path.

- **Dedup `allocate_fresh_local` / `allocate_fresh_shared`.**
  `src/internal/chunk_provider.rs:336-351` vs `441-456`. Identical
  reserve-bytes / allocate / release-on-error scaffolding. Introduce
  `fn allocate_with_budget<F: FnOnce() -> Result<R, AllocError>>(&self,
  total: usize, build: F) -> Result<R, AllocError>` that handles the
  budget rollback. ~20 lines net reduction.

### Speculative (needs perf validation)

- **Collapse the uninit-slice allocation family.** `try_alloc_bytes`,
  `try_alloc_uninit_slice`, `try_alloc_uninit_slice_prefixed` in
  `src/internal/chunk_mutator.rs:245-320` share "compute size, reserve,
  convert ticket" with different ticket shapes. Could be split into a
  low-level `reserve_bytes_for_slice` + thin wrappers. Risk: hottest
  alloc paths; even minor codegen drift could move the gungraun
  benchmark numbers. Do not land without before/after callgrind data.
