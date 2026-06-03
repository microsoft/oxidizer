# TODO

- Eliminate vec of chunk mutators. This should be a linked list

- When expanding a vec that's in an oversized chunk, we should return the
previous chunk back to the system allocator instead of holding onto it in the
retired chunk list.

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
