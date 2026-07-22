# Plurality — Unimplemented Ideas & Future Work

These are designs and ideas carried over from the original design plan that are
**not yet implemented**. The shipped architecture is documented in
[`DESIGN.md`](./DESIGN.md). Items here range from a fully settled design
(`KeyedPool`) to smaller follow-ups.

## 1. `KeyedPool` — copyable generational keys (`keys` feature)

A dedicated, **keyed-only** sibling type that hands out copyable, generational
`Key`s instead of owning `Box`/`Arc` pointers. It reuses the chunked store,
directory, growth, and `!Sync` single-allocator model, but is a *separate type*
(`KeyedPool`) rather than extra methods on `Pool` — that separation is what lets
its slots reach `slotmap`'s size while staying address-stable.

### Why a separate type (not methods on `Pool`)

Keys are the *non-owning* complement to `Box`/`Arc`:

- **Cyclic graphs without leaks.** `Arc` cycles never reach refcount 0; keys
  have no refcounts, so cycles / doubly-linked lists / parent pointers just work.
- **Borrow-checker decoupling.** Hold many `Copy` keys, borrow one (or several
  disjoint) slots at access time.
- **Compact, `Copy`, serializable.** A small integer handle, freely duplicated
  into edge lists and serializable across save/load.

Mixing keys with `Box`/`Arc` on the *same slots* would be unsound if the
metadata word were overloaded (a stale key could alias a slot reused as an
`Arc`, reading its refcount as a generation). Keeping them separate also means a
keyed slot needs neither a refcount nor a stored in-chunk `index` (the `Key`
carries the index; access is index → directory → slot, never bare-pointer
recovery).

### Slot layout — 4 bytes, matching slotmap

```rust
union KeyedValue<T> { value: ManuallyDrop<T>, next_free: u32 }
#[repr(C)]
struct KeyedSlot<T> {
    u: KeyedValue<T>, // value when occupied; next-free global index when free
    version: u32,     // generation; parity encodes occupied (odd) / vacant (even)
}
```

Per-slot overhead is **4 bytes** (`version`) — same as `slotmap::SlotMap` —
because the free list is threaded through the value union (requires
`size_of::<T>() >= 4`). The `directory` and `ChunkHeader` (`base_index`) are
unchanged; `get(key)` maps `key.index` to a slot exactly like `Pool`'s `pop`.

### Handle: the `Key` trait + newtype pattern

`KeyedPool<K, T, A>` is parameterized by a key *type*, where `K` is a user
newtype implementing a `Key` trait — slotmap's "custom key type" idea, but made
the **default one-step pattern** rather than an opt-in.

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct RawKey { /* index: u32, generation: u32, pool_id: u32 */ }
impl RawKey { pub fn to_bits(self) -> u64; pub fn from_bits(bits: u64) -> Self; }

pub trait Key: Copy + Eq + core::hash::Hash {
    fn from_raw(raw: RawKey) -> Self;
    fn into_raw(self) -> RawKey;
}
// RawKey: Key too, as a quick-prototyping escape hatch (the DefaultKey analog).
```

### Construction: `keyed_pool!` / `new_key_type!` macros

A single macro declares the key newtype *and* builds a correctly-typed pool, so
the easiest thing to write is also the safe, distinctly-typed thing:

```rust
keyed_pool! {
    key NodeKey;                                  // declares the newtype + impl Key
    let mut graph: Node = builder().chunk_size(1024).max_chunks(64);
}

let a: NodeKey = graph.insert(node);  // returns NodeKey
graph.get(a);                          // ok
// other_graph.get(a)                  // compile error across key types
```

A declaration-only form for when the key type must be `pub`/cross-module:

```rust
new_key_type! { pub struct NodeKey; }
let mut graph = KeyedPool::<NodeKey, Node>::builder().chunk_size(1024).build();
```

The macros forward doc comments and visibility; generic code can be written over
`<K: Key, T>`.

### API (generic over the key type)

```rust
impl<K: Key, T, A: Allocator> KeyedPool<K, T, A> {
    pub fn insert(&mut self, value: T) -> K;
    pub fn try_insert(&mut self, value: T) -> Result<K, AllocError>;
    pub fn insert_with(&mut self, f: impl FnOnce(K) -> T) -> K; // self-referential

    pub fn get(&self, key: K) -> Option<&T>;
    pub fn get_mut(&mut self, key: K) -> Option<&mut T>;
    pub fn contains_key(&self, key: K) -> bool;
    /// Mutate several distinct slots at once (None if any key is dead or two
    /// keys are equal).
    pub fn get_disjoint_mut<const N: usize>(&mut self, keys: [K; N]) -> Option<[&mut T; N]>;

    pub fn remove(&mut self, key: K) -> Option<T>;

    pub fn len(&self) -> u64;
    pub fn iter(&self) -> impl Iterator<Item = (K, &T)>;
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (K, &mut T)>;
}
```

**Why `&mut self` on the mutators.** With `Copy`, non-owning keys and no RAII,
`&mut self` on `insert`/`remove`/`get_mut` is what makes the model sound: it
forbids holding a `&T` from `get` across a `remove` that frees that slot. `get`
/ `contains_key` stay `&self`. (Mirrors `slotmap`.)

### Cross-pool key identity — layered

1. **Per-key-*type*, compile-time (the `Key` trait + macro).** Different key
   types can't be mixed; the macro makes a fresh distinct type the default
   one-liner. Zero runtime cost.
2. **Per-pool-*instance*, runtime `debug_assert` (a `pool_id` in `RawKey`).**
   The residual case the type system can't catch — two pools of the *same* key
   type cross-using a key — is caught with a unique `pool_id` embedded in every
   key and checked in debug builds. Zero release cost; keys stay plain `Copy`,
   serializable POD. On deserialize, persist+restore the `pool_id` with the pool
   so loaded keys still validate.

An opt-in **generative-lifetime brand** (`KeyedPool<'id, …>`, GhostCell-style)
could provide per-instance protection at *compile* time for users who don't need
serialization — but it is not the default because its invariant lifetime infects
every type that stores a key and defeats key serialization.

### Generation / staleness

`version` is bumped on `remove`; `get` returns `None` on mismatch (stale key).
Parity (odd = occupied) doubles as the liveness flag. Generation wraps after
2³² reuses of a single slot (negligible; a 64-bit generation is available if
desired).

### Positioning vs `slotmap`

`KeyedPool` is **"slotmap with stable addresses."** It is *not* trying to beat
`DenseSlotMap` on iteration (values are scattered across chunks; iteration is a
per-chunk scan that skips holes, unordered). What it offers that slotmap
structurally cannot — because slotmap stores values in one `Vec` that
reallocates and moves values on growth:

- **Stable value addresses** → `Pin` support, stable raw pointers for FFI,
  intrusive / self-referential structures.
- **Incremental, non-copying growth** → no O(N) realloc copy, no transient 2×
  memory spike; predictable latency for large maps.
- **Custom allocator** and a **hard capacity cap** (`max_chunks` → graceful
  `AllocError`).
- One `no_std` crate / allocator story shared with `Pool`.

Build it only if the stable-address niche is a goal; for plain high-frequency
iterate-all-each-tick (ECS) workloads, point users at `slotmap`/`DenseSlotMap`.

## 2. Batch allocation — `alloc_*_many` / `alloc_*_with_many` (push API)

Bulk allocation that hands each new handle to a consumer closure one at a time,
as a complement to every singular `alloc_*` call. A batch can be made faster
than N singular calls by amortizing the free-list pop CAS (claim a whole segment
in one `compare_exchange` instead of one per object), bulk-growing once for the
whole demand and handing out a freshly-grown chunk's slots by direct pointer bump
(no free-list round-trip, contiguous value writes), and hoisting per-call
overhead out of the loop. The win scales with the batch size and is largest for
the `Alloc` flavor, whose per-item work is otherwise just a pop + value write.

### Shape — mirrors the singular `alloc` / `alloc_with` split

Each variant takes a `count`, initializes every object internally (exactly as the
singular calls do), and delivers each handle to an output closure `each` that
**replaces the singular `Alloc<T>` return value** — one instance at a time, no
output buffer. The two forms mirror the singular value source:

- **`_many`** keeps the singular `value: T` and clones it `count` times
  (`T: Clone`) — just as `alloc(value)` takes one value.
- **`_with_many`** takes an index-aware `make(i)` closure — just as
  `alloc_with(f)` takes a maker. Only the flavors that have a `_with` variant
  (all of them) get a `_with_many`.

```rust
// Alloc<T> flavor (lifetime-bound); the others mirror this exactly.

// mirrors `alloc(value)` — one template value, cloned for each object.
pub fn alloc_many(&self, value: T, count: usize, each: impl FnMut(Alloc<'_, T, A>))
where
    T: Clone;

pub fn try_alloc_many(&self, value: T, count: usize, each: impl FnMut(Alloc<'_, T, A>)) -> Result<(), AllocError>
where
    T: Clone;

// mirrors `alloc_with(f)` — each value produced from its index.
pub fn alloc_with_many(&self, count: usize, make: impl FnMut(usize) -> T, each: impl FnMut(Alloc<'_, T, A>));

pub fn try_alloc_with_many(&self, count: usize, make: impl FnMut(usize) -> T, each: impl FnMut(Alloc<'_, T, A>))
    -> Result<(), AllocError>;
```

`each` and `make` must be generic (`impl FnMut`, inlined) — not `&mut dyn FnMut`,
which would cost an indirect call per item and defeat fusing.

### The complement across all flavors

```rust
// owned
alloc_box_many / try_alloc_box_many            (value, count, each: FnMut(Box<T, A>))        where T: Clone
alloc_box_with_many / try_alloc_box_with_many  (count, make, each: FnMut(Box<T, A>))
// shared (atomic)
alloc_arc_many / try_alloc_arc_many            (value, count, each: FnMut(Arc<T, A>))        where T: Clone, A: Clone
alloc_arc_with_many / try_alloc_arc_with_many  (count, make, each)                           where A: Clone
// lifetime-bound
alloc_many / try_alloc_many                    (value, count, each: FnMut(Alloc<'_, T, A>))  where T: Clone
alloc_with_many / try_alloc_with_many          (count, make, each)
// shared (non-atomic)
alloc_rc_many / try_alloc_rc_many              (value, count, each: FnMut(Rc<T, A>))         where T: Clone
alloc_rc_with_many / try_alloc_rc_with_many    (count, make, each)
```

16 methods (4 flavors × {`_many`, `_with_many`} × {panicking, `try_`}). The
`uninit` family is intentionally left out (no batch uninit tier for now).

### Semantics

- **`count` is an explicit parameter** in every variant.
- **Reserved up front.** The implementation claims all `count` slots
  (segment-claim + bulk grow) *before* calling `each`, which makes the fallible
  contract clean and keeps the `AllocError` currency.
- **`try_*` is all-or-nothing:** `Ok(())` ⇒ all `count` delivered;
  `Err(AllocError)` ⇒ **zero** delivered. The pool may still have grown (kept) on
  `Err` — all-or-nothing applies to *delivery*, not to capacity.
- **Panicking variants** panic via `pool_full()` if `count` cannot be reserved,
  like the singular forms.
- **Order** is ascending `0..count`; value `i` (a clone of `value`, or `make(i)`)
  and `each(handle_i)` interleave deterministically.
- **Panic safety.** If `Clone`, `make`, or `each` panics mid-batch, an RAII guard
  splices the undelivered tail back onto the free list in one CAS — no capacity
  leak (same guarantee required of the `*_with` methods).
- **Reentrancy is safe.** Because the segment is atomically detached, a nested
  `alloc*` inside `each` sees a consistent free list and cannot touch in-flight
  slots; a handle the consumer drops is properly freed and becomes reusable.

### Implementation notes

- Snapshot `slot.next` (the free-list link) *before* invoking `each`: once the
  consumer owns the handle, dropping it runs `push_free`, which overwrites that
  field. (The bulk-grow path sidesteps this — the next slot is pointer
  arithmetic, independent of the freed slot's mutated fields.)
- For `Box`/`Arc`/`Rc`, the per-item `pool_refcount` bump can be batched into a
  single `fetch_add(count)` over the reserved segment.

### Usage sketches

```rust
// `_with_many`: generate from the index, link each as it arrives — no buffer.
let mut prev = None;
pool.alloc_with_many(
    n,
    |i| Node::new(i),
    |node| { node.link_after(prev); prev = Some(node); },
);

// `_many`: allocate `n` clones of a template, push each handle into your structure.
pool.alloc_box_many(template, n, |b| sink.push(b));
```

## 3. Guaranteed in-place construction — `alloc_*_emplace` (initializer closure)

`alloc(value)` and `alloc_with(|| value)` both produce a `T` *by value* and then
move it into the slot, so whether construction lands directly in the pool memory
is a best-effort optimizer outcome (RVO/NRVO), never a language guarantee — the
same limitation `std`'s `Box::new`/`Arc::new` have. The only guaranteed escape
today is `alloc_uninit` + write + `assume_init`, which is `unsafe` and clunky.

`alloc_*_emplace` wraps that into a guaranteed-in-place closure: the pool
reserves the slot and hands its raw memory to an **initializer that writes
through a pointer** instead of returning a value. The construction site *is* the
destination — no stack temporary, no caller-visible `assume_init`. This is the
`pin-init` pattern (as used by Rust-for-Linux for `Box::pin_init`/`Arc::pin_init`,
precisely because `Box::new` cannot place large pinned structs).

### Soundness constraint — the API cannot be a naive safe `fn`

The obvious signature is **unsound as a safe `fn`** and must not ship as one:

```rust
// UNSOUND if `pub fn`: nothing forces `init` to initialize the slot.
pub fn alloc_emplace(&self, init: impl FnOnce(&mut MaybeUninit<T>)) -> Alloc<'_, T, A>;
```

Internally the pool reserves via `try_alloc_uninit`, runs `init` on the slot's
`&mut MaybeUninit<T>`, then calls `assume_init`. But `&mut MaybeUninit<T>` is a
safe type that carries **no obligation to write anything**, so this is 100% safe
caller code that fabricates a `T` from uninitialized memory (UB):

```rust
pool.alloc_emplace(|_slot| {}); // wrote nothing → assume_init on garbage
```

A safe function reachable to UB from safe input is unsound. (The `*_with`
methods are fine because a returned-by-value `T` is initialized by construction;
that guarantee is exactly what the raw-pointer initializer drops.) The internal
`assume_init` is unsafe either way — that is expected and not the issue; the
question is solely whether the *public* signature can be safe. There are two
viable shapes, and item 3 must pick one:

### Shape A — `unsafe fn`, caller-initializes contract

```rust
// Alloc<T> flavor; the others mirror this exactly.
//
// # Safety
// `init` must fully initialize the slot (leave a valid `T`) before returning.
pub unsafe fn alloc_emplace(&self, init: impl FnOnce(&mut MaybeUninit<T>))
    -> Alloc<'_, T, A>;

pub unsafe fn try_alloc_emplace(&self, init: impl FnOnce(&mut MaybeUninit<T>))
    -> Result<Alloc<'_, T, A>, AllocError>;
```

Minimal and closure-shape-preserving, at the cost of pushing the initialization
proof onto the caller as an `unsafe` obligation.

### Shape B — safe `fn` with a proof-carrying initializer

Keep the API safe by making full initialization a *type-system* requirement: the
closure receives an uninit guard and can only return the proof token by writing
the value (the `pin-init` approach, under its macros).

```rust
// `Init<'_, T>` is only constructible by writing through the `Uninit<'_, T>`,
// so a returning closure has provably initialized the slot.
pub fn alloc_emplace(&self, init: impl FnOnce(Uninit<'_, T>) -> Init<'_, T>)
    -> Alloc<'_, T, A>;
```

A runtime-checked variant — `FnOnce(&mut MaybeUninit<T>) -> &mut T` with the pool
asserting the returned reference aliases the slot — is also safe, trading a cheap
guard for a slightly looser contract.

In every shape the RAII uninit handle frees the slot if `init` panics — no
capacity leak (same guarantee as the `*_with` methods).

### The complement across all flavors

```rust
alloc_box_emplace   / try_alloc_box_emplace    (init: FnOnce(&mut MaybeUninit<T>)) -> Box<T, A>
alloc_arc_emplace   / try_alloc_arc_emplace    (init) -> Arc<T, A>    where A: Clone
alloc_emplace       / try_alloc_emplace        (init) -> Alloc<'_, T, A>
alloc_rc_emplace    / try_alloc_rc_emplace     (init) -> Rc<T, A>
```

8 methods (4 flavors × {panicking, `try_`}).

### Why an initializer closure beats `_with`

- **Guaranteed in place.** `init` writes through the pointer; there is no
  by-value return slot for the optimizer to (maybe) elide.
- **Composes.** Nested structs can be initialized field-by-field directly in the
  slot, so a large aggregate never exists on the stack as a whole.
- **`Pin`-friendly.** Because the value never moves after construction, this is
  the natural home for pinned / self-referential init — pairs with the existing
  `assume_init_pin`. A future `alloc_pin_emplace` returning a pinned handle is the
  obvious extension.

### Open questions

- **Fallible init.** A second tier `init: FnOnce(&mut MaybeUninit<T>) -> Result<(), E>`
  (freeing the slot and surfacing `E` on error) would mirror `pin-init`'s
  fallible initializers; deferred until a user needs it.
- **Batch.** `alloc_*_emplace_many(count, init: impl FnMut(usize, &mut MaybeUninit<T>))`
  is the guaranteed-in-place batch form — the single-closure `uninit` tier that
  item 2 intentionally left out. It composes the segment-claim reservation of
  item 2 with the emplace contract here.
- **Ergonomics.** Writing through `&mut MaybeUninit<T>` is more verbose than
  `|| value`; `alloc_with` should remain the default for small/medium `T`, with
  `alloc_emplace` reserved for large or pinned values where the guarantee matters.
