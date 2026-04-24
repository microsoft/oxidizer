# Design: Scoped Dependency Overrides for `Resolver`

## Motivation

`Resolver` resolves each type at most once per scope (with promotion to the
shallowest ancestor that could have built it). There is no way to substitute a
dependency for a specific consumer. Two needs drive this feature:

1. **Testing** — replace a real dependency with a fake/mock for a specific consumer.
2. **Contextual variation** — when A and C both depend on B, supply a different B
   when constructing A while C gets the default B.

Constraints:

- **Compile-time safety** — invalid overrides (wrong type, broken dep link) must
  be rejected by the compiler.
- **Scoped applicability** — an override for "B when constructing A" must not
  leak to unrelated consumers of B.
- **Compatibility** — code that does not use overrides keeps working unchanged.

## Background

Today's resolver:

```
Resolver<T>
├── types: Arc<SharedTypeMap>           // local cache, keyed by TypeId
├── ancestors: Vec<Arc<SharedTypeMap>>  // parent caches
└── depth: Option<usize>                // promotion tracker
```

`get::<O>()` walks local + ancestor caches, recursively resolves `O::Inputs` on
miss, constructs `O`, and promotes it to the shallowest ancestor whose data was
sufficient.

Trait chain: `ResolveFrom<T>` → `ResolutionDeps<T>` → `Resolver<T>`. The
`#[resolvable]` macro emits `impl ResolveFrom<B> for Service` whose `new()`
takes resolved references to the deps.

## User-Facing API

### Fluent `provide` builder

All values are registered via `resolver.provide(value)`, which returns a
builder. Calling `.when_injected_in::<T>()` zero or more times narrows the
scope; the value commits when the builder is dropped.

```rust
// A → B → C dependency chain.

// Unscoped: equivalent to resolver.insert(d). Applies anywhere D is needed.
resolver.provide(d);

// Path-scoped: C is overridden ONLY on the A → B → C path.
// Read bottom-up: "provide c, when injected in B, when injected in A."
resolver.provide(c).when_injected_in::<B>().when_injected_in::<A>();
```

### Branching paths via `either` / `or`

A single `provide()` can register multiple alternative path tails sharing one
value. Crucially, all branches share the same cache identity for the
intermediates on their paths — so the overridden value's effect on shared
intermediates (like B below) is preserved across branches.

```rust
// Override C through B, but only when B is part of A1 or A2.
// The B built with custom C is shared between both consumer paths.
resolver.provide(c)
    .when_injected_in::<B>()
    .either(|x| x.when_injected_in::<A1>())
    .or(|x| x.when_injected_in::<A2>());
```

Branches can themselves contain `when_injected_in` chains and nested
`either` / `or`. The identity branch `|x| x` opts in to also matching the bare
prefix:

```rust
resolver.provide(c)
    .when_injected_in::<B>()
    .either(|x| x)                          // matches [B, C] alone
    .or(|x| x.when_injected_in::<A1>())
    .or(|x| x.when_injected_in::<A2>());
```

### Compile-time safety

Each `.when_injected_in::<T>()` validates that the previous link is a declared
direct dependency of `T`, via the `DependencyOf<Target>` marker trait emitted by
`#[resolvable]`:

```rust
// #[resolvable] generates:
//   impl DependencyOf<B> for C {}   // because B's constructor takes &C

resolver.provide(c)
    .when_injected_in::<B>()         // OK
    .when_injected_in::<Unrelated>() // error[E0277]: B: DependencyOf<Unrelated>
;
```

Same validation applies inside branch closures.

### Resolution

`get()` / `ensure()` are unchanged. Overrides are consulted automatically.

### Override precedence

When multiple registrations could match, **longest matching path wins**. Within
the same path length, last registration wins. An unscoped `provide()` (the
single-element path `[T]`) is the global fallback, identical to `insert()`.

## Internal Mechanics

### The path-keyed cache

The classical `TypeId`-keyed cache is replaced by a **path-keyed slot cache**.

```rust
type Path = Vec<TypeId>;                      // root-first; last element is the type
type Slot = Arc<RwLock<Option<Box<dyn Any + Send + Sync>>>>;

struct PathCache {
    slots: HashMap<Path, Slot>,
    // Index for fast lookup: the last TypeId of each path.
    by_last: HashMap<TypeId, Vec<Path>>,
}
```

Two key properties:

- **Path identity is the cache identity.** Two registrations referring to the
  same path get the same slot (via `entry().or_insert_with()`).
- **Slot is shareable.** Branched `provide()` calls hand out the same `Arc` to
  every branch whose chain passes through a given path.

Classical resolution is a special case: a normally-resolved value of type `T`
lives at the single-element path `[T]`. Lookup with any current path falls back
to `[T]` if no longer-suffix match exists, reproducing today's behavior.

#### Lookup

To resolve `O` with current path `P` (the chain of types currently being
resolved, *not* including `O`):

1. Construct the search key: `P ++ [O]`.
2. Among all cached paths ending in `O`, find the **longest** that is a suffix
   of `P ++ [O]`.
3. If found, that's the slot.

```
Cached paths ending in C: [C], [B, C], [A, B, C], [X, A, B, C]

Resolving C with P = [A, B]   → search key [A, B, C]
  Suffix candidates: [C] ✓, [B, C] ✓, [A, B, C] ✓, [X, A, B, C] ✗ (longer than key)
  Longest: [A, B, C]

Resolving C with P = []       → search key [C]
  Suffix candidates: [C] ✓
  Result: [C]
```

#### Slot allocation by `provide()`

When `provide(value).when_injected_in::<B>().when_injected_in::<A>()` (chain
`A.B.C`) commits, the builder pre-creates a slot for **every prefix path along
the chain**, on the resolver where `provide()` was called:

| Path        | Type | Initial state                 |
|-------------|------|-------------------------------|
| `[A, B, C]` | `C`  | filled with the provided value|
| `[A, B]`    | `B`  | empty (`None`)                |
| `[A]`       | `A`  | empty (`None`)                |

Pre-creating intermediate slots is what makes the override-affected
intermediates cacheable at the right path. When resolution later constructs
`B` on the `A.B.C` path, it finds the empty `[A, B]` slot and fills it.

**Same path → same slot.** If a later `provide()` also wants a slot at, say,
`[A, B]`, `entry([A, B]).or_insert_with(...)` returns the existing `Arc`. This
is how shared intermediates work for both branched single-`provide()` calls and
multiple separate `provide()` calls that happen to need the same intermediate.

For branched `provide()`, each branch produces its own chain; the builder
pre-creates slots for each branch's chain. Where two branches' chains share a
path (e.g. `[B, C]` shared by branches that extend it differently), that shared
path naturally maps to a single slot.

### Resolution flow

Two pieces of context flow through resolution:

- **Current path** (`PathStack`) — required for longest-suffix lookup. Flows
  **downward** as a `&PathStack` parameter through `Resolver::resolve` and
  `ResolutionDeps::resolve_all`.
- **Placement tier** — the resolver level at which the resolved value should be
  cached. Flows **upward** as part of the return value.

No per-resolution mutable state is stored on the resolver. The `ResolverStore`
trait has been removed; `ResolutionDeps::resolve_all` takes `&mut Resolver<T>`
directly. The `#[resolvable]` macro is unchanged.

```rust
pub struct PathStack<'a> { /* borrowed cons cell on the call stack */ }
impl<'a> PathStack<'a> {
    pub fn root() -> Self;
    pub fn push(&'a self, t: TypeId) -> PathStack<'a>;
    pub fn as_slice(&self) -> &[TypeId];
}

pub struct ResolveOutput<'a, O> {
    pub value: &'a O,
    pub tier: usize,
}

impl<T: 'static> Resolver<T> {
    pub fn resolve<O: ResolveFrom<T>>(&mut self, path: &PathStack<'_>) -> ResolveOutput<'_, O>;
}

pub trait ResolutionDeps<T> {
    type Resolved<'r>;
    fn resolve_all(
        store: &mut Resolver<T>,
        path: &PathStack<'_>,
    ) -> (Self::Resolved<'_>, usize); // tuple value + max tier across deps
}
```

Algorithm for `Resolver::resolve::<O>(path)`:

```rust
fn resolve<O>(&mut self, path: &PathStack<'_>) -> ResolveOutput<'_, O> {
    let key: Vec<TypeId> = path.as_slice().iter().copied()
        .chain([TypeId::of::<O>()]).collect();

    // 1. Find the best slot across self + ancestors: longest path ending in O
    //    that is a suffix of `key`. Returns the slot and the owning tier.
    let target = self.find_best_slot::<O>(&key);

    // 2. Slot already filled → return it.
    if let Some((slot, tier)) = &target {
        if let Some(value) = slot.read() {
            return ResolveOutput { value, tier: *tier };
        }
    }

    // 3. Construct O. Push O onto the path for dep resolution.
    let child_path = path.push(TypeId::of::<O>());
    let (inputs, deps_tier) =
        <O::Inputs as ResolutionDeps<T>>::resolve_all(self, &child_path);
    let o = O::new(inputs);

    // 4. Place the value.
    match target {
        // Pre-existing empty slot from a provide() call: fill it. Tier is set
        // by the resolver that owns the slot.
        Some((slot, tier)) => {
            slot.write(o);
            ResolveOutput { value: slot.read(), tier }
        }
        // No matching slot: classical promotion. Allocate at single-element
        // path [O] on the resolver at deps_tier.
        None => {
            let slot = self.resolver_at(deps_tier)
                .insert_slot(vec![TypeId::of::<O>()], o);
            ResolveOutput { value: slot.read(), tier: deps_tier }
        }
    }
}
```

**Why this satisfies both motivating cases:**

- **Child-defined override never leaks to parent.** A `provide()` on the child
  pre-creates slots only on the child's `PathCache`. The parent's
  `find_best_slot` walks only itself and its own ancestors — it never sees the
  child's slots. A parent-level resolution falls back to `[O]` and gets the
  default.
- **Parent-defined override applies to child resolutions and pools at the
  parent.** Slots live on the parent. A child's `find_best_slot` walks up,
  finds them, and uses them — including for intermediates. Filling the slot
  writes through the shared `Arc`, so a subsequent `parent.get::<A>()` finds
  the same instance.

### Promotion rule

Promotion folds out of the algorithm above:

- If a pre-existing slot is found, the value lands at the slot's owning
  resolver level — set by `provide()`'s home level.
- Otherwise, the value lands at `max` of its deps' tiers — the classical rule.

Override-affected values therefore pin to the override's home level
automatically, never escaping above it.

### Suffix-matching examples

```
# provide(c).when_injected_in::<B>().when_injected_in::<A>()
# Slots: [A, B, C] (filled), [A, B] (empty), [A] (empty)

resolver.get::<A>():
  path=[], resolving A → key=[A]. Best slot: [A] (empty).
  Recurse for B with path=[A]:
    key=[A, B]. Best slot: [A, B] (empty).
    Recurse for C with path=[A, B]:
      key=[A, B, C]. Best slot: [A, B, C] (filled). Return c.
    Construct B from c. Fill [A, B]. Return B.
  Construct A from B. Fill [A]. Return A.

resolver.get::<B>():
  path=[], resolving B → key=[B]. No slot named B exists yet.
  Recurse for C with path=[B]:
    key=[B, C]. Slots ending in C: [A, B, C] (not a suffix of [B, C]). None match.
    Construct default C, allocate at [C], return.
  Construct B from default C. Allocate at [B]. Return B.
```

```
# Branched: provide(c).when_injected_in::<B>()
#                    .either(|x| x.when_injected_in::<A1>())
#                    .or(|x| x.when_injected_in::<A2>());
# Slots: [A1, B, C] and [A2, B, C] both filled with c (one shared Arc).
#        [A1, B] and [A2, B] are SEPARATE empty slots.
#        [A1] and [A2] separate empty slots.

resolver.get::<A1>(): fills [A1, B] with B(c) and [A1] with A1.
resolver.get::<A2>(): fills [A2, B] with B(c) and [A2] with A2.
```

The `[A1, B]` and `[A2, B]` slots are distinct because their paths differ;
each branch gets its own B *instance*, but both are constructed from the same
shared C value. To force one shared B instance across A1 and A2, register at
the shorter `[B, C]` path.

### Two `provide()` calls sharing an intermediate

`B` depends on `C1` *and* `C2`. Two provides override different deps of `B` on
the same `A.B` chain:

```rust
provide(c1).when_injected_in::<B>().when_injected_in::<A>(); // chain A.B.C1
provide(c2).when_injected_in::<B>().when_injected_in::<A>(); // chain A.B.C2
```

| Provide | Slots created                              |
|---------|--------------------------------------------|
| 1       | `[A, B, C1]` (filled), `[A, B]`, `[A]`     |
| 2       | `[A, B, C2]` (filled), `[A, B]`, `[A]`     |

Provide 2's `entry([A, B]).or_insert_with(...)` returns the same `Arc`
allocated by provide 1. Same for `[A]`. So `get::<A>()` constructs **one B**
from both `c1` and `c2`, fills the shared `[A, B]` slot once, and that one B
is visible to both overrides' affected paths.

### Why slot sharing must be by *equal* paths only

A weaker rule (e.g. "reuse if the new path is a suffix of an existing slot's
path") would silently merge semantically distinct values:

```
provide #1: chain X.A.B.C → slots include [X, A, B] (empty)
provide #2: chain   A.B.C → wants slot [A, B]
```

The B inside `X.A.B.C` is built under provide #1's overridden C; the B inside
`A.B.C` is built under provide #2's overridden C. They are different values.
Reusing `[X, A, B]` for `[A, B]` would silently corrupt one. The equal-path
rule (HashMap key equality) avoids this — two different paths get two
different slots, and longest-suffix lookup picks the right one per request.

## API Surface

### New / changed types

| Type                                | Purpose |
|-------------------------------------|---------|
| `DependencyOf<Target>`              | Marker trait emitted by `#[resolvable]` for compile-time link validation |
| `ProvideBuilder<'a, T, Dep, Path>`  | Fluent builder returned by `provide()`; commits on drop |
| `BranchBuilder<'b, Path>`           | Short-lived builder passed into `either` / `or` closures |
| `Unscoped`, `Scoped<I, R>`          | Type-level path tags for compile-time validation |
| `PathStack<'a>`                     | Borrowed view of the in-flight resolution chain |
| `ResolveOutput<'a, O>`              | `{ value: &O, tier: usize }` returned by `resolve` |
| `PathCache`                         | Replaces today's `SharedTypeMap`; keys are `Vec<TypeId>` |

The `ResolverStore` trait is removed. `Resolver` and `ResolutionDeps` interact
directly.

### Resolver

| Method | Purpose |
|--------|---------|
| `provide(value)` | Start a `ProvideBuilder` |
| `get::<O>()`     | Existing; calls `self.resolve(&PathStack::root())` internally |
| `resolve::<O>(path)` | Internal entry point used by `ResolutionDeps` |

| Field | Type | Purpose |
|-------|------|---------|
| `types` | `Arc<PathCache>` | Path-keyed cache (replaces `SharedTypeMap`) |
| `ancestors` | `Vec<Arc<PathCache>>` | Parent caches |
| `level` | `usize` | Resolver tier (0 = root, +1 per `scoped()`); seeds slot home level |

The legacy `depth: Option<usize>` field is removed; placement tier flows via
`ResolveOutput`.

### `ProvideBuilder`

| Method | Purpose |
|--------|---------|
| `when_injected_in::<Target>()` | Extend the linear path; validates via `DependencyOf` |
| `either(\|x\| ...)` | Open a branched section; closure builds one alternative |
| `or(\|x\| ...)` | Add another alternative; same mechanics as `either` |
| *(Drop)* | Commit: unscoped → `insert()`; scoped → pre-create slots along each branch's chain on the local resolver |

### `#[resolvable]` macro change

For each constructor parameter, also emit `impl DependencyOf<Self> for DepType {}`.
Backwards-compatible: existing code does not reference `DependencyOf`.

## Examples

### Unscoped provide is `insert()`

```rust
resolver.provide(custom_d);   // identical to resolver.insert(custom_d)
```

### Direct path-scoped override

```rust
// A directly depends on B. Provide B only when constructing A.
resolver.provide(custom_b).when_injected_in::<A>();

resolver.get::<A>();  // uses custom_b
resolver.get::<C>();  // C also depends on B, but uses default B
```

### Chained override

```rust
// A → B → C. Override C only on the A → B → C path.
resolver.provide(custom_c).when_injected_in::<B>().when_injected_in::<A>();

resolver.get::<A>();  // A's B uses custom_c
resolver.get::<B>();  // standalone B uses default C
```

### Multi-path diamond

```rust
// A depends on B AND on E (which also depends on B).
// Override C only on the direct A → B path, not via E.
resolver.provide(custom_c).when_injected_in::<B>().when_injected_in::<A>();

resolver.get::<A>();
// A's direct B: path [A, B] resolving C → matches [A, B, C] → custom_c.
// A's via E:    path [A, E, B] resolving C → no path ending in C is a suffix
//               of [A, E, B, C] → default C used.
// Result: A holds two distinct B instances.
```

### Branched override — shared overridden C across consumers

```rust
resolver.provide(custom_c)
    .when_injected_in::<B>()
    .either(|x| x.when_injected_in::<A1>())
    .or(|x| x.when_injected_in::<A2>());

resolver.get::<A1>();  // A1's B is built with custom_c
resolver.get::<A2>();  // A2's B is built with the same custom_c value
```

### Two overrides sharing an intermediate

```rust
// B has deps C1 and C2.
resolver.provide(c1).when_injected_in::<B>().when_injected_in::<A>();
resolver.provide(c2).when_injected_in::<B>().when_injected_in::<A>();

resolver.get::<A>();
// One B is constructed using both c1 and c2 (shared [A, B] slot).
```

### Parent / child interaction

```rust
let mut parent = Resolver::new(AppBase { ... });
parent.provide(fake_c).when_injected_in::<B>().when_injected_in::<A>();

let mut child = parent.scoped(RequestBase { ... });
child.get::<A>();   // walks up to parent, uses parent's slots, fills them on parent
parent.get::<A>();  // same slot lookup → same A instance

// Reverse case:
let mut other = Resolver::new(AppBase { ... });
let mut other_child = other.scoped(RequestBase { ... });
other_child.provide(fake_c).when_injected_in::<B>().when_injected_in::<A>();

other_child.get::<A>();  // override fires; A cached on the child
other.get::<A>();        // parent never sees the child's slots; default A
```

### Compile-time errors

```rust
resolver.provide(custom_c).when_injected_in::<Unrelated>();
// error[E0277]: the trait bound `C: DependencyOf<Unrelated>` is not satisfied
```

## Alternatives Considered

### Override via a scoped resolver

```rust
let child = resolver.scoped(OverrideBase { validator: fake });
child.get::<Client>();
```

Rejected: leaks to all consumers in the child scope; requires defining new
base types per override combination; no compile-time validation.

### Override set passed at resolution time

```rust
resolver.get_with::<Client>(OverrideSet::with::<Client, Validator>(fake));
```

Rejected: must be rebuilt at every call site; cannot share cache with the
normal path; inconsistent return type (owned vs. `&O`).

### `(TypeId, BTreeSet<OverrideEntryId>)` cache key

An earlier draft keyed the cache on `(TypeId, set of overrides that fired
below)`. Rejected: required a `BTreeSet`-typed cache key, runtime computation
of "which overrides will fire in this subtree" before construction, and an
extra `OverrideEntryId` indirection. The path-keyed slot cache subsumes all of
this with a `Vec<TypeId>` key and straightforward longest-suffix lookup.

### Reuse-by-suffix at slot allocation

Reusing an existing slot whose path has the new path as a suffix would
silently merge semantically distinct values built under different override
contexts. The equal-path rule avoids this. See "Why slot sharing must be by
equal paths only" above.

## Implementation Plan

### Phase 1: Core types and `DependencyOf`

- Add `DependencyOf<T>` and the `Unscoped` / `Scoped` path tags.
- Update `#[resolvable]` to emit `DependencyOf` impls.
- Existing tests continue to pass.

### Phase 2: `PathCache` and resolution-flow changes

- Replace `SharedTypeMap` with `PathCache`.
- Drop the `ResolverStore` trait; thread `&PathStack` and `ResolveOutput`
  through `Resolver::resolve` and `ResolutionDeps::resolve_all`.
- Drop the `depth` field; use `tier` from `ResolveOutput`.
- Verify classical resolution (no `provide()`) is observationally identical.

### Phase 3: Linear `provide` builder

- Add `Resolver::level` and `provide()`.
- Implement linear `when_injected_in()` with `DependencyOf` bounds.
- On `Drop`, pre-create slots for every prefix path along the chain on the
  caller's resolver, with the value occupying the leaf.
- Tests: unscoped provide, single-link override, chained override, multi-path
  diamond, parent/child cases.

### Phase 4: Branching via `either` / `or`

- Add `BranchBuilder` and `either` / `or`.
- Each branch produces its own chain; slot pre-creation handles the rest.
- Tests: branched overrides, shared intermediates across `provide()` calls,
  identity branches (`|x| x`).

### Phase 5: Polish

- Documentation, cross-crate tests, optional batch `provide!` macro.

## Open Questions

1. **Inheritance into scoped resolvers.** A `scoped()` child does not get a
   copy of the parent's `provide()` registrations on its own cache — but parent
   overrides still apply via the standard ancestor walk. A
   `scoped_with_path_overrides()` variant could copy entries down if explicit
   inheritance ever becomes useful.

2. **Clearing overrides.** No removal API initially; users create a fresh
   resolver. Could be added if test-teardown patterns demand it.

3. **Lookup cost.** `find_best_slot::<O>` consults the per-resolver
   `by_last[TypeId(O)]` index, then suffix-checks each candidate path against
   the search key. Linear in the number of cached paths ending in `O` per
   resolver, times the chain depth for the suffix check. A trie over reversed
   paths is a future optimization if it ever matters.

4. **Concurrent fills.** Slots are `Arc<RwLock<Option<...>>>`. Two concurrent
   resolutions racing to fill the same slot must agree: the writer should
   check `is_none()` under the lock and either install or drop the second
   value.
