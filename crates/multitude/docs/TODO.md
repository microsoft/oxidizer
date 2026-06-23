# TODO

## General

- Consider introducing arena-friendly hash map and hash set

- `alloc<T: Send>` requires `T: Send` because `T::drop` runs at arena
  reset/drop and `Arena: Send`, so teardown may run on a migrated thread.
  This is an undesirable constraint. We could introduce a Mode generic to
  Arena to control whether it is `Send` or `!Send` and adjust the constraint
  on `alloc<T>` accordingly.

- No owning IntoIterator for Box<[T]> (std has it). Minor, but an easy ergonomic win.

- Consider storing the length of arrays in the chunk using a variable integer encoding instead
  of always storing a usize. This would save RAM and CPU cache space, at the cost of a bit of computation
  whenever getting the length.

- Add tests with allocation_tracker to make sure allocation promises are
in fact maintained.

## Optional freeze-prefix reservation for `Vec`/`String`/`Utf16String`

Every growable buffer currently reserves the `Arc<[T]>` freeze prefix
(`[strong][len]`) unconditionally, so `into_arc` / `into_boxed_slice` are
zero-copy (see `vec/freeze.rs`, `internal::constants::buffer_freezable`,
`ArenaBuf::freeze_prefix`). That costs a few prefix bytes (and the
`arc_block_align` rounding) on every buffer — even ones that are never
frozen.

Let the caller choose, at construction time, whether to reserve the prefix,
based on how they intend to use the collection: buffers that won't be frozen
skip the prefix and pack tighter, while freeze-bound buffers keep O(1)
`into_arc` / `into_boxed_slice`.

Two shapes (same underlying "buffer may or may not carry the prefix" work,
which already exists via the `freeze_prefix` flag and the const
`buffer_freezable` gate):

- **Runtime flag on the builders** (`alloc_vec*` / `alloc_string*` /
  `alloc_utf16_string*`): record the choice in `ArenaBuf` next to the
  existing `freeze_prefix` flag and branch in `Vec::try_grow_to`. Smallest
  API; one branch in the cold growth path; freeze falls back to the O(n)
  copy when the prefix is absent (the `can_freeze_in_place` check already
  handles this).
- **Zero-cost marker type parameter on `Vec`**: a sealed marker selecting
  prefix-vs-no-prefix, with `into_arc` / `into_boxed_slice` O(1) only on the
  freeze-ready variant. No runtime branch and a compile-time freeze
  guarantee; cost is generic noise in signatures, mitigated by defaulting to
  today's behavior plus type aliases.

`String` / `Utf16String` wrap `Vec`, so the choice propagates for free.
