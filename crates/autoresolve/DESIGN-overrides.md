# Design: Scoped Dependency Overrides for `Resolver`

## Motivation

Today, `Resolver` resolves each type at most once per scope (with promotion to the shallowest ancestor that could have built it). There is no way to substitute a dependency when resolving a specific consumer. Two concrete needs drive this feature:

1. **Testing** — Replace a real dependency with a fake/mock for a specific consumer without affecting the rest of the graph.
2. **Contextual variation** — When service A and service C both depend on B, supply a different B instance only when constructing A, while C gets the default B.

The design must preserve the crate's core properties:

* **Compile-time safety** — Invalid overrides (wrong type, missing override, type mismatch) must be caught by the compiler.
* **Scoped applicability** — An override for "B when constructing A" must not leak to unrelated consumers of B.
* **Compatibility** — Existing code without overrides must continue to work unchanged.

## Background: How Resolution Works Today

```
Resolver<T>
├── types: Arc<SharedTypeMap>          // local scope
├── ancestors: Vec<Arc<SharedTypeMap>> // parent scopes
└── depth: Option<usize>              // promotion tracker
```

When `resolver.get::<Service>()` is called:
1. Check local and ancestor caches.
2. If not found, resolve `Service::Inputs` recursively, construct `Service`.
3. Promote the result to the shallowest ancestor whose data was sufficient.

Key trait chain: `ResolveFrom<T>` → `ResolutionDeps<T>` → `ResolverStore<T>`.

The `#[resolvable]` macro generates a blanket `impl<B> ResolveFrom<B> for Service` bounded on each dependency also implementing `ResolveFrom<B>`.

## Design

### Core Concept: Unified Value Model

The resolver already manages values through `SharedTypeMap` — base types inserted at creation, and resolved types cached after construction. The `provide()` feature introduces **one new concept**: path-scoped values. Unscoped values are handled by the existing storage.

Concretely, there are two kinds of stored values:

- **Unscoped values** — base types, resolved types, and globally-provided values all live in the existing `SharedTypeMap`. Calling `resolver.provide(value)` without any `.when_injected_in()` is **identical to `resolver.insert(value)`** — same storage, same code path. From the resolver's perspective, a globally-provided value is indistinguishable from a base type or a normally-resolved cached value.

- **Path-scoped values** — keyed on a resolution path (an ordered sequence of types through the dependency graph). These are the only new storage concept. A path-scoped value is used when the registered path matches a **suffix** of the current resolution stack:
  - `[B, C]` — "whenever resolving B's dependency C, substitute it — regardless of what is above B in the dependency chain."
  - `[A, B, C]` — "when resolving C, but only when B is being resolved as part of A. When B is resolved directly (not as part of A), use the default C."

Because unscoped `provide()` is literally `insert()`, the mental model is simple: **everything goes into the same store unless you scope it**.

### User-Facing API

#### Fluent `provide` builder

All values are registered through a single entry point — `resolver.provide(value)` — which returns a builder. The builder optionally narrows scope via `.when_injected_in::<T>()` calls. Each call adds one link to the path, validated at compile time. The value is committed to the resolver when the builder is dropped.


```rust
// Assume: A depends on B, B depends on C.
let mut resolver = autoresolve::Resolver::new(base);

let c: C = /* custom value */;
let d: D = /* custom value */;

// Provide C only when resolving it through A → B → C.
// Read bottom-up: "provide c, when injected in B, when injected in A."
resolver.provide(c).when_injected_in::<B>().when_injected_in::<A>();

// Provide D unconditionally (equivalent to insert) — any resolution path that needs D will use this value.
resolver.provide(d);
```

#### How the builder works

The builder is a temporary object with an `&mut Resolver` borrow. Each `.when_injected_in::<T>()` consumes the builder and returns a new one with an extended path type. When the final builder is dropped (at the semicolon), the value is committed to the resolver.

```rust
resolver.provide(c)           // ProvideBuilder<C, Unscoped>
    .when_injected_in::<B>()     // ProvideBuilder<C, Scoped<B, Unscoped>>
    .when_injected_in::<A>();    // ProvideBuilder<C, Scoped<A, Scoped<B, Unscoped>>>
                                 // dropped → registers path [A, B, C]
```

Without any `.when_injected_in()` calls, the builder commits an **unscoped value** — it goes directly into `SharedTypeMap` via `insert()`, identical to a base type:

```rust
resolver.provide(d);          // ProvideBuilder<D, Unscoped>
                                 // dropped → calls insert(d) into SharedTypeMap
```

#### Resolving — no API change

The existing `get()` / `ensure()` API is unchanged. Path-scoped values are consulted automatically:

```rust
let a = resolver.get::<A>();  // A → B uses custom c (via chained override), any D uses custom d
let b = resolver.get::<B>();  // B uses default C (chain override doesn't match), any D uses custom d
```

#### Override precedence

When both a path-scoped value and an unscoped value exist for the same type, the **most specific match wins**:

1. **Longest suffix match** in `path_overrides` takes priority. If both `[B, C]` and `[A, B, C]` match the current stack, `[A, B, C]` wins (more specific).
2. Shorter suffix match in `path_overrides` is the next fallback.
3. Unscoped value in `SharedTypeMap` (base type, resolved cache, or global `provide()`) is the final fallback.

```rust
let c_for_a: C = /* special */;
let c_global: C = /* fallback */;

resolver.provide(c_for_a).when_injected_in::<B>().when_injected_in::<A>();
resolver.provide(c_global);  // equivalent to: resolver.insert(c_global);

resolver.get::<A>();  // A → B → C uses c_for_a (path match)
resolver.get::<B>();  // B → C uses c_global   (unscoped fallback from SharedTypeMap)
```

#### Compile-time safety

Each `.when_injected_in::<T>()` call validates that the previous type in the chain is a declared dependency of `T`, using the `DependencyOf<Target>` marker trait generated by `#[resolvable]`.

```rust
// Given: impl B { fn new(c: &C) -> Self }
// Generated: impl DependencyOf<B> for C {}

resolver.provide(c)
    .when_injected_in::<B>()   // OK: C: DependencyOf<B>
    .when_injected_in::<A>();  // OK: B: DependencyOf<A>

resolver.provide(c)
    .when_injected_in::<Unrelated>();  // error[E0277]: C: DependencyOf<Unrelated> not satisfied
```

For unscoped provides (no `.when_injected_in()`), only the value type is checked — no dependency relationship is required since the value applies unconditionally.

### Internal Mechanics

#### Storage

The resolver gains one new field for path-scoped values and a resolution-tracking stack:

```rust
pub struct Resolver<T> {
    types: Arc<SharedTypeMap>,
    ancestors: Vec<Arc<SharedTypeMap>>,
    depth: Option<usize>,
    path_overrides: PathOverrideMap,     // NEW — path-scoped values only
    resolution_stack: Vec<TypeId>,       // NEW — tracks the current resolution path
    taint_depth: Option<usize>,          // NEW — shallowest override root in the active resolution
    base: PhantomData<T>,
}
```

Unscoped values (base types, resolved types, and unscoped `provide()` calls) all live in the existing `SharedTypeMap`. The only new storage is for path-scoped values:

```rust
/// Stores path-scoped override values. The key is the full resolution path
/// including the dep type as the last element.
pub(crate) struct PathOverrideMap {
    inner: HashMap<Vec<TypeId>, Box<dyn Any + Send + Sync>>,
}

impl PathOverrideMap {
    /// Insert a path-scoped value.
    pub fn insert(&mut self, path: Vec<TypeId>, value: Box<dyn Any + Send + Sync>) {
        self.inner.insert(path, value);
    }

    /// Look up a path-scoped value for dep type `Dep` given the current resolution stack.
    /// Uses **suffix matching**: finds the longest registered path whose prefix (all elements
    /// except the final dep type) matches a suffix of `stack`. Returns both the value and
    /// the stack index of the override root (used for taint tracking).
    pub fn get<Dep: Send + Sync + 'static>(&self, stack: &[TypeId]) -> Option<(&Dep, usize)> {
        let dep_id = TypeId::of::<Dep>();
        let mut best: Option<(&Box<dyn Any + Send + Sync>, usize, usize)> = None;

        for (path, value) in &self.inner {
            // Path must end with the dep type.
            if path.last() != Some(&dep_id) {
                continue;
            }
            let prefix = &path[..path.len() - 1]; // override path without dep
            if prefix.len() > stack.len() {
                continue;
            }
            // Check if prefix matches a suffix of the stack.
            let stack_suffix = &stack[stack.len() - prefix.len()..];
            if stack_suffix == prefix {
                // Keep the longest (most specific) match.
                if best.as_ref().map_or(true, |b| path.len() > b.1) {
                    let root_depth = stack.len() - prefix.len();
                    best = Some((value, path.len(), root_depth));
                }
            }
        }

        best.and_then(|(value, _, root_depth)| {
            value.downcast_ref::<Dep>().map(|v| (v, root_depth))
        })
    }

    /// Returns true if any registered override path could fire within `O`'s subtree,
    /// given the current resolution stack. This is used to decide whether to bypass
    /// the cache for `O` — if an override exists deeper in `O`'s dependency chain,
    /// a cached `O` may be stale and must be re-resolved.
    pub fn has_subtree_override<O: 'static>(&self, stack: &[TypeId]) -> bool {
        let o_id = TypeId::of::<O>();
        // Build the hypothetical stack if we were to push O.
        // Check if any override path's prefix starts with a suffix of (stack + [O]).
        for path in self.inner.keys() {
            let prefix = &path[..path.len() - 1]; // override path without dep
            if prefix.is_empty() {
                continue;
            }
            // Check: does (stack + [O]) end with a non-empty prefix-of-prefix?
            // i.e., for some k in 1..=prefix.len(), the last k elements of
            // (stack ++ [O]) equal prefix[..k].
            let extended_len = stack.len() + 1;
            for k in 1..=prefix.len().min(extended_len) {
                let override_slice = &prefix[..k];
                let start = extended_len - k;
                let matches = override_slice.iter().enumerate().all(|(i, &tid)| {
                    let idx = start + i;
                    if idx < stack.len() { stack[idx] == tid } else { o_id == tid }
                });
                if matches {
                    return true;
                }
            }
        }
        false
    }
}
```

This is the complete new storage. There is no separate "global override" map — unscoped `provide(value)` calls `insert(value)` directly into `SharedTypeMap`, sharing the same code path as base type insertion.

#### Builder type system

The builder carries the dep value, a mutable reference to the resolver, and a type-level path:

```rust
/// Path not yet scoped — value goes into SharedTypeMap via insert().
pub struct Unscoped;

/// Path scoped to `Immediate`, with further ancestors in `Rest`.
/// Reads as: "dep is injected into Immediate, which is itself in Rest."
pub struct Scoped<Immediate, Rest>(PhantomData<(Immediate, Rest)>);

/// Builder returned by `resolver.provide(value)`.
/// Dropped to commit the value. Consumed by `.when_injected_in()` to extend the path.
pub struct ProvideBuilder<'a, T, Dep, Path> {
    resolver: &'a mut Resolver<T>,
    value: Option<Dep>,    // Option so Drop can take()
    _path: PhantomData<Path>,
}
```

**Entry point on `Resolver`:**

```rust
impl<T: Send + Sync + 'static> Resolver<T> {
    pub fn provide<Dep>(&mut self, value: Dep) -> ProvideBuilder<'_, T, Dep, Unscoped>
    where
        Dep: Send + Sync + 'static,
    {
        ProvideBuilder {
            resolver: self,
            value: Some(value),
            _path: PhantomData,
        }
    }
}
```

**First `.when_injected_in()` — validates dep → consumer link:**

```rust
impl<'a, T, Dep> ProvideBuilder<'a, T, Dep, Unscoped> {
    pub fn when_injected_in<Target>(mut self) -> ProvideBuilder<'a, T, Dep, Scoped<Target, Unscoped>>
    where
        Dep: DependencyOf<Target>,
        Target: Send + Sync + 'static,
    {
        let value = self.value.take();
        // Prevent Drop from committing the consumed builder.
        std::mem::forget(self);
        ProvideBuilder {
            resolver: /* carried from self */,
            value,
            _path: PhantomData,
        }
    }
}
```

**Subsequent `.when_injected_in()` — validates previous-immediate → new-target link:**

```rust
impl<'a, T, Dep, Immediate, Rest> ProvideBuilder<'a, T, Dep, Scoped<Immediate, Rest>> {
    pub fn when_injected_in<Target>(mut self) -> ProvideBuilder<'a, T, Dep, Scoped<Target, Scoped<Immediate, Rest>>>
    where
        Immediate: DependencyOf<Target>,
        Target: Send + Sync + 'static,
    {
        // Same pattern: take value, forget self, return new builder.
    }
}
```

> **Note on move semantics:** The builder uses `Option::take()` + `std::mem::forget()` to transfer the value and resolver reference without triggering `Drop` on intermediate builders. Only the final builder's `Drop` commits the value. An alternative is `ManuallyDrop` wrapping.

**Drop — commits the value:**

```rust
/// Extracts the runtime path from a type-level path.
trait OverridePath {
    /// None = unscoped (insert into SharedTypeMap), Some([...]) = path-scoped (root-first).
    fn to_path() -> Option<Vec<TypeId>>;
}

impl OverridePath for Unscoped {
    fn to_path() -> Option<Vec<TypeId>> { None }
}

impl<Immediate: 'static, Rest: OverridePath> OverridePath for Scoped<Immediate, Rest> {
    fn to_path() -> Option<Vec<TypeId>> {
        // Scoped<A, Scoped<B, Unscoped>> → path [A, B] (root-first).
        // A is the outermost consumer (added last by the user),
        // B is the immediate consumer of Dep.
        let mut path = Rest::to_path().unwrap_or_default();
        path.insert(0, TypeId::of::<Immediate>());
        Some(path)
    }
}

impl<'a, T, Dep, Path> Drop for ProvideBuilder<'a, T, Dep, Path>
where
    Dep: Send + Sync + 'static,
    Path: OverridePath,
{
    fn drop(&mut self) {
        if let Some(value) = self.value.take() {
            match Path::to_path() {
                None => {
                    // Unscoped — same as insert(). Goes into SharedTypeMap.
                    self.resolver.insert(value);
                }
                Some(mut path) => {
                    // Path-scoped — append the dep type to complete the key.
                    path.push(TypeId::of::<Dep>());
                    self.resolver.path_overrides.insert(path, Box::new(value));
                }
            }
        }
    }
}
```

**Compile-time validation summary — the type checking for each call:**

```
resolver.provide(c: C)                   // Dep = C, Path = Unscoped
    .when_injected_in::<B>()                // requires C: DependencyOf<B>
                                            // Path becomes Scoped<B, Unscoped>
    .when_injected_in::<A>()                // requires B: DependencyOf<A>
                                            // Path becomes Scoped<A, Scoped<B, Unscoped>>
;                                           // Drop: path = [A, B], dep = C → key = [A, B, C]
```

#### Resolution flow with path-scoped values

When `resolver.get::<O>()` is called, the resolver maintains a **resolution stack** that tracks the full path of types currently being resolved. Path-scoped values are matched using **suffix matching** against this stack, and a **taint depth** tracks whether intermediate results can safely be cached.

The modified resolution flow in `ResolverStore::resolve`:

```rust
fn resolve<O: ResolveFrom<T>>(&mut self) -> &O {
    // 1. Check if O itself has a path-scoped value matching a suffix of the current stack.
    if let Some((value, root_depth)) = self.path_overrides.get::<O>(&self.resolution_stack) {
        // Update taint depth to the shallowest override root seen so far.
        self.taint_depth = Some(match self.taint_depth {
            Some(d) => d.min(root_depth),
            None => root_depth,
        });
        self.mark(0); // force local tier — prevents promotion
        return /* reference to the overridden value */;
    }

    // 2. Check if any override could fire deeper in O's subtree.
    //    If so, we must skip the cache — a cached O may have been built without the override.
    let has_subtree_overrides = self.path_overrides.has_subtree_override::<O>(
        &self.resolution_stack,
    );

    if !has_subtree_overrides {
        // 3. No overrides apply — safe to use cache (existing fast path).
        if let Some(cached) = self.types.get::<O>() /* or ancestor lookup */ {
            return cached;
        }
    }

    // 4. Resolve O's dependencies. Push O onto the resolution stack first.
    let my_depth = self.resolution_stack.len();
    self.resolution_stack.push(TypeId::of::<O>());

    // --- resolve deps, construct O (existing slow path) ---

    self.resolution_stack.pop();

    // 5. Caching decision based on taint depth.
    let tainted = self.taint_depth.map_or(false, |td| my_depth > td);

    if tainted {
        // O is between the override root and the overridden dep —
        // a standalone get::<O>() would produce a different result.
        // Do NOT cache in SharedTypeMap.
        // Store in a temporary tainted_values map for the duration of the parent's resolution.
    } else {
        // O is at or above the override root — cache normally.
        // A standalone get::<O>() would trigger the same overrides.
        if self.taint_depth == Some(my_depth) {
            self.taint_depth = None; // clear taint — we've exited the override root
        }
        // ... store in SharedTypeMap and promote as usual ...
    }
}
```

The lookup order is:

1. **Path-scoped override for O** — suffix match against the current `resolution_stack`. If found, use the provided value directly.
2. **Subtree override check** — if any override could fire within O's dependency subtree, skip the cache and re-resolve O from scratch so the deeper overrides can fire.
3. **Existing cache** (SharedTypeMap local + ancestors) — only consulted when no subtree overrides apply. Covers base types, previously resolved types, and unscoped `provide()` values.
4. **Normal resolution** — resolve dependencies recursively, construct O.

**How suffix matching works:**

The resolution stack records the chain of types being actively resolved. Override paths are matched against suffixes of this stack:

```
# Override registered: [A, B, C]

resolver.get::<A>()          → stack: [A]       → resolving A's deps
  └─ resolving B (dep of A)  → stack: [A, B]    → resolving B's deps
       └─ resolving C (dep of B) → suffix match: stack [A, B], override prefix [A, B]
                                   [A, B] is a suffix of [A, B] ✓ → override fires
```

```
# Override registered: [B, C]  (no A in the path)

resolver.get::<A>()          → stack: [A]       → resolving A's deps
  └─ resolving B (dep of A)  → stack: [A, B]    → resolving B's deps
       └─ resolving C (dep of B) → suffix match: stack [A, B], override prefix [B]
                                   [B] is a suffix of [A, B] ✓ → override fires
```

A standalone call:

```
# Override registered: [A, B, C]

resolver.get::<B>()          → stack: [B]       → resolving B's deps
  └─ resolving C (dep of B)  → suffix match: stack [B], override prefix [A, B]
                                 [A, B] is NOT a suffix of [B] ✗ → override does NOT fire
```

If no path-scoped match is found, the resolver falls through to the `SharedTypeMap` lookup, which returns the unscoped value (whether it was a base type, previously resolved, or globally provided via `provide()`).

#### Interaction with caching

Path-scoped values introduce **context-dependent resolution** — the same type can produce different instances depending on where it appears in the dependency graph. The caching rules ensure that context-dependent results do not pollute the global cache.

**Core rule:** A type T can be cached in `SharedTypeMap` if and only if every override that fired during T's construction would also fire for a standalone `get::<T>()`. The test: **does the override path start with T (i.e., T is the override root)?**

- Override `[B, C]` — root is B. When B is resolved (regardless of context), this override fires because suffix matching always matches when B is on the stack. **B can be cached** — standalone `get::<B>()` produces the same result.
- Override `[A, B, C]` — root is A. When B is resolved as part of A, the override fires. But standalone `get::<B>()` would NOT trigger it. **B must not be cached** — it is tainted.

**Taint tracking:** The resolver tracks a `taint_depth: Option<usize>` — the shallowest stack index of any override root that fired during the current resolution. When a type T finishes resolving at stack depth `d`:

- If `d > taint_depth` — T is tainted (between the override root and the overridden dep). Do not cache in `SharedTypeMap`. Store in a temporary `tainted_values` map that is cleared after the override root finishes resolving.
- If `d == taint_depth` — T is the override root. **Cache normally.** Clear the taint (or reduce to the next-shallowest active taint if multiple overrides overlap).
- If `d < taint_depth` — cannot happen (taint is always within the current resolution subtree).

**Cache bypass on read:** When resolving type T, the resolver checks whether any registered override could fire within T's subtree (via `has_subtree_override`). If so, the cache is **bypassed** and T is resolved from scratch — otherwise a previously cached T (built without the override) would be returned, and the deeper override would never fire.

**Multiple instances are intentional:** When a type appears in A's dependency graph through multiple paths and an override applies to only one path, A's graph will contain distinct instances of that type — one built with the override, one without. This is the intended behavior: path overrides create **path-local instances**, not scope-wide substitutions. The `.when_injected_in()` API makes this explicit — the user is opting into a different instance for a specific path.

**Examples:**

```
# Override: [A, B, C] — "C when injected in B when injected in A"
# Dependency graph: A → B → C, A → E → B → C

Path A → B → C:
  Stack [A, B], resolving C → suffix [A, B] matches [A, B] ✓
  Override fires. Taint depth = 0 (A's position).
  C = overridden value.
  B is at depth 1 > taint_depth 0 → tainted, NOT cached.
  A is at depth 0 == taint_depth 0 → cached ✓, taint cleared.

Path A → E → B → C:
  Stack [A, E, B], resolving C → suffix [A, B] does NOT match [A, E, B]
  (the last 2 elements are [E, B], not [A, B])
  Override does NOT fire. B is resolved normally with default C.
  B can be cached (no taint from this path).
```

```
# Override: [B, C] — "C whenever injected in B"
# Same dependency graph: A → B → C, A → E → B → C

Path A → B → C:
  Stack [A, B], resolving C → suffix [B] matches [A, B] ✓
  Override fires. Taint depth = 1 (B's position, since override root is B).
  C = overridden value.
  B is at depth 1 == taint_depth 1 → cached ✓, taint cleared.

Path A → E → B → C:
  B already cached (with overridden C) → returns cached B. ✓
  Same result as standalone get::<B>().
```

Unscoped `provide()` values are already in `SharedTypeMap` (they're inserted via `insert()`), so caching works exactly as it does for base types — no special handling needed.

#### Interaction with promotion

Path-scoped dependencies and tainted intermediate types must **not** be promoted to ancestor scopes. The value is specific to this resolver's override configuration and should not leak to sibling resolvers.

Two mechanisms prevent promotion:

1. **Direct overrides** — when a path-scoped value is injected, the resolution logic forces tier 0 (local):

```rust
if let Some((value, root_depth)) = self.path_overrides.get::<O>(&self.resolution_stack) {
    self.taint_depth = Some(/* ... */);
    self.mark(0); // force local — prevents promotion
    return /* reference to value */;
}
```

2. **Tainted intermediates** — types between the override root and the overridden dep are stored in the temporary `tainted_values` map, not in `SharedTypeMap`, so they cannot be promoted.

The **override root itself** (the type that starts the override path) is cached in `SharedTypeMap` with its tier determined by the override's forced tier 0. Since one of its transitive deps is tier 0, the root's depth will be at most 0 — it stays local. This naturally prevents the root and its override-affected subtree from being promoted to parent scopes.

Unscoped `provide()` values participate in promotion normally — they're in `SharedTypeMap` and behave exactly like base types.

#### Interaction with scoped resolvers

When a scoped resolver is created via `scoped()`, path-scoped values are **not inherited** by default. Each resolver manages its own `path_overrides`:

```rust
let mut parent = Resolver::new(AppBase { ... });
parent.provide(fake_validator).when_injected_in::<Client>();

let mut child = parent.scoped(RequestBase { ... });
// child does NOT inherit the (Client, Validator) path override.
// child.get::<Client>() resolves Validator normally.
```

Unscoped `provide()` values, being in `SharedTypeMap`, participate in the normal ancestor lookup — they **are** visible to child scopes, just like base types.

If path override inheritance is desired, the user explicitly re-registers overrides on the child. This is intentional — path-scoped values are per-resolver configuration, and implicit inheritance could cause surprising behavior in nested scopes.

**Future consideration**: A `scoped_with_path_overrides()` variant could copy the parent's path overrides into the child. This is additive and can be introduced later.

### Compile-Time Enforcement of Override Validity

#### Type correctness (automatic)

The builder's `value: Dep` parameter and `Send + Sync + 'static` bounds catch type mismatches at the entry point. This is free.

#### Dependency relationship validation (per-link)

Every `.when_injected_in::<T>()` call validates one link via the `DependencyOf<Target>` marker trait generated by `#[resolvable]`:

```rust
/// Marker trait: `Self` is a declared (direct) dependency of `Target`.
pub trait DependencyOf<Target> {}
```

The `#[resolvable]` macro generates impls alongside `ResolveFrom`:

```rust
// Given: impl Client { fn new(validator: &Validator, clock: &Clock) -> Self }
// Generated:
impl DependencyOf<Client> for Validator {}
impl DependencyOf<Client> for Clock {}
```

The first `.when_injected_in::<B>()` requires `Dep: DependencyOf<B>`. Each subsequent `.when_injected_in::<T>()` requires `PreviousImmediate: DependencyOf<T>`. This validates every link in the chain at compile time, with errors pointing directly at the offending call.

Unscoped provides (no `.when_injected_in()`) have no dependency constraint — they apply unconditionally and are equivalent to `insert()`.

### Syntactic Sugar (not needed initially)

The builder API is already concise. A macro could allow batch registration:

```rust
autoresolve::provide!(resolver,
    Validator => fake_validator,                           // unscoped (= insert)
    Clock [in Client] => fake_clock,                      // path-scoped
    ServiceC [in ServiceB, in ServiceA] => custom_c,      // chained path-scoped
);
```

Not required for the initial implementation.

## API Surface Summary

### New traits

| Trait | Purpose |
|---|---|
| `DependencyOf<Target>` | Marker generated by `#[resolvable]` — asserts direct dependency relationship |
| `OverridePath` | (internal) Maps type-level path (`Unscoped` / `Scoped<...>`) to runtime `Vec<TypeId>` |

### New types

| Type | Purpose |
|---|---|
| `ProvideBuilder<'a, T, Dep, Path>` | Fluent builder returned by `provide()`, committed on drop |
| `Unscoped` | Type-level tag — value goes into SharedTypeMap (via `insert()`) |
| `Scoped<Immediate, Rest>` | Type-level cons-list — value goes into `path_overrides` for a specific resolution path |
| `PathOverrideMap` | (internal) Map storing path-scoped values keyed by resolution path |

### New methods on `Resolver<T>`

| Method | Signature | Purpose |
|---|---|---|
| `provide` | `fn provide<Dep>(&mut self, value: Dep) -> ProvideBuilder<'_, T, Dep, Unscoped>` | Start building a provide — unscoped = `insert()`, scoped = path override |

### Methods on `ProvideBuilder`

| Method | Signature | Purpose |
|---|---|---|
| `when_injected_in` | `fn when_injected_in<Target>(self) -> ProvideBuilder<..., Scoped<Target, ...>>` | Narrow scope — validates link via `DependencyOf` |
| *(Drop)* | implicit | Commits: unscoped → `insert()` into SharedTypeMap, scoped → `path_overrides` |

### Changes to `Resolver<T>` struct

| Field | Type | Purpose |
|---|---|---|
| `path_overrides` | `PathOverrideMap` | Stores path-scoped values keyed by resolution path |
| `resolution_stack` | `Vec<TypeId>` | Tracks the current resolution chain during `resolve()` |
| `taint_depth` | `Option<usize>` | Shallowest override root in the active resolution (for cache bypass) |

### Changes to `#[resolvable]` macro

For each dependency parameter, generate an additional:
```rust
impl DependencyOf<Self> for DepType {}
```

This is backwards-compatible — existing code doesn't use `DependencyOf` and the new impls don't conflict.

## Examples

### Unscoped provide — equivalent to `insert()`

```rust
let mut resolver = autoresolve::Resolver::new(base);

// These two are identical — both insert into SharedTypeMap:
resolver.provide(custom_d);
resolver.insert(custom_d);   // same effect

resolver.get::<ServiceA>();  // if ServiceA (transitively) needs D, uses custom_d
resolver.get::<ServiceB>();  // same — custom_d everywhere
```

### Direct path-scoped provide — A gets custom B, C gets default B

```rust
#[resolvable]
impl ServiceA {
    fn new(b: &ServiceB) -> Self { ... }
}

#[resolvable]
impl ServiceC {
    fn new(b: &ServiceB) -> Self { ... }
}

let mut resolver = autoresolve::Resolver::new(base);

// "Provide ServiceB when it's injected in ServiceA."
resolver.provide(custom_b).when_injected_in::<ServiceA>();

let a = resolver.get::<ServiceA>();  // uses custom_b
let c = resolver.get::<ServiceC>();  // resolves ServiceB normally
```

### Chained path-scoped provide — override a transitive dep for one path only

```rust
#[resolvable]
impl ServiceA {
    fn new(b: &ServiceB) -> Self { ... }
}

#[resolvable]
impl ServiceB {
    fn new(c: &ServiceC) -> Self { ... }
}

let mut resolver = autoresolve::Resolver::new(base);

// "Provide ServiceC when injected in ServiceB, when injected in ServiceA."
// → only the A → B → C path gets custom_c.
resolver.provide(custom_c).when_injected_in::<ServiceB>().when_injected_in::<ServiceA>();

let a = resolver.get::<ServiceA>();  // A's B uses custom_c. A is cached. B is NOT cached (tainted).
let b = resolver.get::<ServiceB>();  // B uses the default C (override root is A, not B).
                                     // This B is resolved fresh and cached normally.
```

### Suffix matching — context-independent override

```rust
#[resolvable]
impl ServiceA {
    fn new(b: &ServiceB) -> Self { ... }
}

#[resolvable]
impl ServiceB {
    fn new(c: &ServiceC) -> Self { ... }
}

let mut resolver = autoresolve::Resolver::new(base);

// "Provide ServiceC whenever injected in ServiceB" — no matter what is above B.
// Override path is [B, C]. Root is B.
resolver.provide(custom_c).when_injected_in::<ServiceB>();

let a = resolver.get::<ServiceA>();  // A → B → C: suffix [B] matches stack [A, B] → custom_c used.
                                     // B is the override root → B is cached. A is cached.
let b = resolver.get::<ServiceB>();  // B → C: suffix [B] matches stack [B] → custom_c used.
                                     // Returns the cached B (same instance as inside A).
```

### Multi-path diamond — path-scoped overrides create path-local instances

```rust
#[resolvable]
impl ServiceA {
    fn new(b: &ServiceB, e: &ServiceE) -> Self { ... }
}

#[resolvable]
impl ServiceE {
    fn new(b: &ServiceB) -> Self { ... }
}

#[resolvable]
impl ServiceB {
    fn new(c: &ServiceC) -> Self { ... }
}

let mut resolver = autoresolve::Resolver::new(base);

// Override C only on the A → B → C path (not the A → E → B → C path).
resolver.provide(custom_c).when_injected_in::<ServiceB>().when_injected_in::<ServiceA>();

let a = resolver.get::<ServiceA>();
// A's direct dep B: stack [A, B], resolving C → suffix [A, B] matches → custom_c.
//   This B is tainted (override root is A, B is deeper) → NOT cached.
// A's dep E: stack [A, E], resolving B: stack [A, E, B], resolving C
//   → suffix [A, B] does NOT match [A, E, B] → default C.
//   This B is resolved normally → cached in SharedTypeMap.
// Result: A holds two distinct ServiceB instances — one with custom_c (direct),
//         one with default C (via E). This is intentional.

let b = resolver.get::<ServiceB>();  // Returns the cached B (with default C).
```

### Combining unscoped and path-scoped values

```rust
resolver.provide(fallback_c);  // unscoped: equivalent to insert(fallback_c)
resolver.provide(special_c).when_injected_in::<ServiceB>().when_injected_in::<ServiceA>();

let a = resolver.get::<ServiceA>();  // A → B → C uses special_c (longest suffix match wins)
let b = resolver.get::<ServiceB>();  // B → C uses fallback_c (unscoped from SharedTypeMap)
let x = resolver.get::<ServiceX>();  // X → ... → C uses fallback_c (unscoped from SharedTypeMap)
```

### Compile-time errors

```rust
// C is not a dependency of Unrelated:
resolver.provide(custom_c).when_injected_in::<Unrelated>();
// error[E0277]: the trait bound `ServiceC: DependencyOf<Unrelated>` is not satisfied

// Each link is validated — B is not a dependency of Unrelated:
resolver.provide(custom_c)
    .when_injected_in::<ServiceB>()    // OK: ServiceC: DependencyOf<ServiceB>
    .when_injected_in::<Unrelated>();  // error[E0277]: ServiceB: DependencyOf<Unrelated> not satisfied
```

## Alternatives Considered

### Alternative 1: Override via scoped resolver

Use the existing `scoped()` mechanism: create a child scope, insert the override value, resolve.

```rust
let child = resolver.scoped(OverrideBase { validator: fake_validator });
let client = child.get::<Client>();
```

**Rejected** because:
- The override leaks to all types resolved in the child scope, not just Client.
- Requires defining a new base type for every override combination.
- No compile-time validation that the override is relevant to Client.

### Alternative 2: Override set passed at resolution time

```rust
let client = resolver.get_with::<Client>(
    OverrideSetEnd.and::<Client, Validator>(fake_validator)
);
```

**Rejected** because:
- Overrides must be rebuilt for every `get()` call — verbose and error-prone.
- Return type must be owned (`O`) instead of `&O` since the result can't be cached without poisoning the normal path.
- Inconsistent API: `get()` returns `&O` but `get_with()` returns `O`.

### Alternative 3: Separate methods per chain depth

```rust
resolver.override_dep::<Client, Validator>(fake_validator);          // direct
resolver.override_chain::<A, B, C>(custom_c);                        // depth 2
resolver.override_path::<Chain<A, Chain<B, Chain<C, D>>>>(custom_d); // arbitrary
```

**Rejected** because:
- Three different methods for the same concept — inconsistent API surface.
- The `Chain` cons-list type for deeper overrides is awkward to read and write.
- The fluent `.when_injected_in()` builder unifies all depths into a single pattern.

### Alternative 4: Shadow type map

Allow inserting "shadow" values that take precedence during the next resolution call.

**Rejected** because:
- Stateful and easy to forget to clear.
- No target scoping — shadows affect all consumers.

## Implementation Plan

### Phase 1: Core types and `DependencyOf` generation
- Add `DependencyOf<T>` trait.
- Update `#[resolvable]` macro to emit `DependencyOf` impls.
- Add `PathOverrideMap`, `Unscoped`, `Scoped` types.
- All existing tests continue to pass (no behavioral changes).

### Phase 2: `ProvideBuilder` and resolution integration
- Add `path_overrides`, `resolution_stack`, and `taint_depth` fields to `Resolver`.
- Implement `provide()` returning `ProvideBuilder`.
- Implement `when_injected_in()` with `DependencyOf` bounds and `Drop` commit logic.
  - Unscoped drop calls existing `insert()`.
  - Scoped drop inserts into `path_overrides`.
- Modify `resolve()` to:
  - Check path overrides with suffix matching before resolving deps.
  - Check for subtree overrides and bypass cache when applicable.
  - Push/pop the resolution stack around dependency resolution.
  - Track taint depth when overrides fire; skip caching for tainted intermediates.
  - Store tainted intermediates in a temporary map, not `SharedTypeMap`.
- Ensure path-scoped values and tainted intermediates prevent promotion (force tier 0).
- Add unit tests for unscoped provides, direct path-scoped provides, chained path-scoped provides, multi-path diamond scenarios, and cache correctness.

### Phase 3: Ergonomics and documentation
- Add documentation and examples.
- Add cross-crate override tests in the test project.
- Consider batch `provide!` macro.

## Open Questions

1. **Duplicate path registrations** — Registering the same path twice: should the second call replace the first (last-wins), or panic? Last-wins is simpler and mirrors `HashMap::insert` semantics.

2. **Path override inheritance in scoped resolvers** — Should child resolvers created via `scoped()` inherit the parent's `path_overrides`? The current design says no (explicit re-registration required). Unscoped values are naturally inherited because they're in `SharedTypeMap` (visible as ancestors). A `scoped_with_path_overrides()` variant could be added later.

3. **Clearing path overrides** — Should there be a method to remove a previously registered path override? Useful for test setup/teardown. Not required initially — users can create a fresh resolver instead.

4. ~~**Caching with path-scoped values**~~ — **Resolved.** Intermediate types between the override root and the overridden dep are **not cached** in `SharedTypeMap` (they are tainted). The override root itself is cached. This ensures that a standalone `get::<B>()` after `get::<A>()` (where `[A, B, C]` was overridden) resolves B fresh with the default C. See "Interaction with caching" for full rules.

5. **Path-scoped value cloning** — The current design clones path-scoped values when injecting them (since the same path override may apply to multiple resolution calls in child scopes). Should `Dep: Clone` be required? Or should path-scoped values be wrapped in `Arc` internally to avoid the clone bound?
