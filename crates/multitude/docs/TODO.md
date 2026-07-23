# TODO

## General

- No owning IntoIterator for Box<[T]> (std has it). Minor, but an easy ergonomic win.

- Consider storing the length of arrays in the chunk using a variable integer encoding instead
  of always storing a usize. This would save RAM and CPU cache space, at the cost of a bit of computation
  whenever getting the length.

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

## Guaranteed in-place initialization

`alloc(value)` and the `alloc_*_with(|| value)` family produce a `T` by value
and then write it into the reserved arena slot. LLVM commonly elides the
temporary, but Rust does not guarantee that construction occurs directly at the
destination. Large values can therefore consume stack space or be moved before
reaching their final address. The existing `alloc_uninit*` plus `assume_init`
APIs provide guaranteed placement, but require callers to use `unsafe`.

Add a safe, `pin-init`-style initializer abstraction and corresponding
`alloc_*_emplace` methods for all ownership flavors:

- `alloc_emplace` / `try_alloc_emplace`
- `alloc_box_emplace` / `try_alloc_box_emplace`
- `alloc_arc_emplace` / `try_alloc_arc_emplace`
- `alloc_rc_emplace` / `try_alloc_rc_emplace`

Pinned forms should initialize directly into the final address and return a
`Pin` without ever moving `T`. Fallible initializers should release any acquired
chunk reference and propagate their own error while preserving the arena's
documented panic/allocation-failure behavior.

A plain `FnOnce(&mut MaybeUninit<T>)` is **not** a sound safe API: the closure
can return without initializing the slot. The design must use a typed
initializer whose contract guarantees complete initialization (with safe
combinators for field-by-field construction), or keep the initialization
contract explicitly `unsafe`. It must also define:

- behavior when initialization panics after reserving bump space;
- whether fallible initialization can reclaim the reservation or leaves it
  occupied until reset;
- initialization of dynamically sized values and slices;
- interaction with strong-count and metadata prefixes for `Arc` / `Rc`;
- whether batch emplacement is useful after singular emplacement exists.

Keep `alloc_*_with` as the ergonomic default. Emplacement is primarily for
large aggregates, pinned/self-referential values, and code that requires a
language-level no-move guarantee. Benchmark those cases, including peak stack
usage, before choosing the final API.
