# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Initial implementation of `Pool<T, A>`: a growable, chunked, fixed-slot object
  pool (`Send + !Sync`, single-producer / multi-consumer).
- `PoolBuilder` for configuring chunk size, an optional chunk cap, and a custom
  allocator.
- Four single-pointer-wide handle flavors, all dereferencing to `&T`:
  - `Box<T>` — unique, `'static`, may outlive the pool.
  - `Alloc<'pool, T>` — unique, borrows the pool; skips the pool reference count
    entirely (cheapest), at the cost of being `!Send` and not outliving the pool.
  - `Arc<T>` — shared, atomic refcount, `Send + Sync`.
  - `Rc<T>` — shared, **non-atomic** refcount, `!Send + !Sync`; ~3× faster
    clone/drop for single-threaded sharing (sound because an occupied slot is
    never accessed atomically by another thread; validated with Miri).
- Allocation API per flavor: `alloc*` / `*_with` closure variants / `*_uninit`
  placement variants with `assume_init`, plus fallible `try_*` siblings.
- `Arc::get_mut` / `Rc::get_mut` for uniqueness-checked mutable access.
- `no_std` support (requires `alloc`); lock-free internals using only
  `core::sync::atomic`.
- `benches/graph_churn.rs`: a graph add/remove benchmark comparing the pool
  against `std::Box` on mimalloc over an identical, checksum-verified op stream.
- `benches/alloc_callgrind.rs`: deterministic instruction-count benchmarks of
  the allocate/free hot paths via gungraun (Valgrind/Callgrind).
- Test infrastructure: Miri-clean unit/integration tests, [bolero] property/fuzz
  tests over random op streams, and [loom] concurrency-permutation tests (atomics
  indirected through `src/atomic.rs`) covering the lock-free free list, per-slot
  refcounts, and cross-thread teardown.

[bolero]: https://docs.rs/bolero
[loom]: https://docs.rs/loom

### Performance

- Hot/cold split guided by the gungraun (Callgrind) benchmarks: every
  allocator-touching path (`grow`, `teardown`, `build`) is `#[cold]`/
  `#[inline(never)]`, and the hot handle methods (`alloc`/`free`/`clone`/
  `deref`) are `#[inline]`. This lets `alloc_slot` inline into the allocation
  loop.
- Hot-path micro-optimizations: unchecked directory indexing on `pop` (the
  index is provably in bounds), a single `index` read with inlined header
  recovery on `free`, and relaxed memory orderings on the free-list push
  (validated with loom).
- Together these cut the allocate+free hot paths by **~32–42%** in instruction
  count versus the first working version, and the graph benchmark runs ~2.4×
  faster than `std::Box` on mimalloc.

### Known limitations / follow-ups

- No early per-chunk reclamation (chunks live until pool teardown).
- No `Weak` handle.
