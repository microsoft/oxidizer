# TODO

The forward-looking backlog of open work for the `multitude` crate. It records
only what is still worth doing: completed items are deleted rather than marked
done, so the length of this file is a real measure of outstanding work. The
shipped architecture is documented in [`DESIGN.md`](./DESIGN.md).

## Contents

### Features
- [F1](#f1) — Provide an owning `IntoIterator` for `Box<[T]>`
- [F2](#f2) — Add guaranteed in-place initialization (`alloc_*_emplace`)
- [F3](#f3) — Add `ArenaSnapshot<T>` for single-owner immutable graphs

## Features

<a id="f1"></a>
### F1 — Provide an owning `IntoIterator` for `Box<[T]>`

**Area:** `boxed` · **Priority:** Low · **Effort:** Small

No owning IntoIterator for Box<[T]> (std has it). Minor, but an easy ergonomic win.

**Done when:** `multitude::Box<[T], A>` implements `IntoIterator` with an
owning iterator that yields `T` by value, runs the destructor of any element
not consumed, and releases the chunk reference when the iterator drops.

---

<a id="f2"></a>
### F2 — Add guaranteed in-place initialization (`alloc_*_emplace`)

**Area:** `arena` · **Priority:** Medium · **Effort:** Large

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

**Done when:** the emplace family exists for all four ownership flavors with a
signature that cannot fabricate an uninitialized `T` from safe code, each of
the listed open behaviors is documented, an unwinding initializer leaks no
chunk reference, and a benchmark shows the reduced peak stack usage for a
large aggregate.

---

<a id="f3"></a>
### F3 — Add `ArenaSnapshot<T>` for single-owner immutable graphs

**Area:** `arena` · **Priority:** Low · **Effort:** Large

Today, a graph that must outlive its source `Arena` generally uses
escape-capable owners throughout the graph:

```rust
struct Document<A: Allocator + Clone> {
    title: multitude::Box<str, A>,
    sections: multitude::Box<[multitude::Box<Section<A>, A>], A>,
}

struct Section<A: Allocator + Clone> {
    heading: multitude::Box<str, A>,
    body: multitude::Box<str, A>,
}
```

Each box is independently safe to move outside the arena, but each allocation
must participate in chunk ownership. Large immutable trees can therefore carry
ownership metadata and perform reference-count bookkeeping at many nodes even
when the application only ever retains or drops the entire tree as one unit.

An immutable region snapshot would instead retain all relevant chunks through
one root owner. Values inside the region could refer directly to other values
in the same region:

```rust
struct Document<'region> {
    title: &'region str,
    sections: &'region [Section<'region>],
}

struct Section<'region> {
    heading: &'region str,
    body: &'region str,
}
```

The conceptual construction API would produce an owning handle:

```rust
let document: ArenaSnapshot<Document<'_>> = ArenaSnapshot::build(|arena| {
    // Parse and allocate the complete graph in `arena`.
    build_document(arena, input)
})?;

process(document.root());
```

`ArenaSnapshot<T>` would contain ownership of the frozen chunk set plus a
pointer to the root `T`. Moving `ArenaSnapshot<T>` would move only the handle;
the root and all referenced values would remain at stable addresses in the
retained chunks. Borrowing `document.root()` would tie every internal reference
to the owning handle, so a child string or section could not outlive the
snapshot.

The intended benefits are:

- one ownership boundary for an entire immutable graph;
- lightweight internal references instead of a `Box`, `Arc`, or `Rc` at every
  edge;
- bulk teardown of the graph's chunks;
- lower metadata and reference-count overhead for large trees that are never
  split into independently owned pieces.

This is not merely a wrapper around `Arena`. It is an owning-self-reference:
the handle owns the storage containing `T`, while `T` contains references into
that same storage. A safe design must answer:

- how construction returns a root reference without allowing it to escape
  independently of the eventual owner;
- how the arena is sealed so no later allocation can invalidate assumptions;
- how chunks used by pre-existing escape-capable `Box`, `Arc`, or `Rc` values
  interact with the snapshot;
- which destructors run when the snapshot is dropped, especially for values
  reached only through references;
- how failed construction drops initialized values and releases every chunk;
- whether serialization, DSTs, custom allocators, and thread transfer are
  supported;
- whether root and child pointers are stored directly or represented by
  internal offsets/handles during construction.

Possible implementation shapes include:

1. an `ArenaSnapshot::build` closure using higher-ranked lifetimes, with an
   internal unsafe step that captures the root pointer and retained chunks;
2. a builder that returns an opaque root token rather than a Rust reference,
   then resolves that token after freezing;
3. a `FrozenArena` owning the chunks separately, with typed handles resolved
   through a borrow of the frozen arena.

The snapshot would be immutable and would not support allocating more related
data after freezing. Because this design establishes a new unsafe ownership
abstraction, proceed only if benchmarks show meaningful savings over the
existing graph of escape-capable smart pointers, and require a dedicated
soundness review before implementation.

**Done when:** a benchmark first establishes the saving over an equivalent
graph of escape-capable smart pointers; then, if pursued, one implementation
shape ships with every listed design question answered in the rustdoc, a root
reference that cannot escape its snapshot, correct teardown on both successful
and failed construction, and a recorded soundness review.

**See also:** F2 (in-place initialization of graph nodes during construction)
