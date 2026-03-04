# Autoresolve Spec: Compile-Time Dependency Injection

## 1. Problem Statement

The current API (`basic.rs`) requires a single base type `T` for the entire hierarchy (`Resolver<Builtins>`) and manual `impl ResolveFrom<Builtins> for DerivedType` with boilerplate destructuring. This creates two problems:

1. **Single root limitation** — real hierarchies have multiple independent roots (`Builtins`, `Telemetry`, `Request` in `combined.rs`).
2. **Boilerplate** — every derived type needs a hand-written `ResolveFrom` impl with nested `ResolutionDepsNode` types and pattern matching.

## 2. Design Goals

| Goal | How |
|------|-----|
| Compile-time safety | Missing/unsatisfiable dependencies → compiler error, not runtime panic |
| Caching | Each dependency constructed at most once; shared across all consumers |
| User friendliness | Declare dependencies via `#[resolvable]` on existing `impl` blocks |
| Reasonable errors | Missing dep → trait-not-satisfied with transitive notes; cycle → overflow with cycle trace |

## 3. Architecture

### Existing Trait Hierarchy (unchanged)

- `ResolveFrom<T>` — "I can be built given a `Resolver<T>`." Associated type `Inputs` (a heterogeneous type-list of dependencies) and `fn new(resolved_inputs) -> Self`.
- `ResolutionDeps<T>` — "I am a type-list of dependencies resolvable from `T`." Methods: `ensure()` (eagerly insert all deps), `get_private()` (retrieve references).
- `ResolutionDepsNode<H, T>` / `ResolutionDepsEnd` — cons/nil for the heterogeneous list.
- `Resolver<T>` — holds a `TypeMap`, resolves types on demand via `get::<O>()` requiring `O: ResolveFrom<T>`.

### New Components

- **`#[resolvable]`** — proc macro attribute on `impl` blocks. Generates `impl<B> ResolveFrom<B> for Type where deps: ResolveFrom<B>`.
- **`resolver!()`** — declarative macro at the call site. Generates a phantom base type + base-type `ResolveFrom` impls, returns `Resolver<PhantomBase>`.

### Why the Blanket Impl Must Go

The existing blanket:

```rust
impl<T> ResolveFrom<T> for T where T: Clone + Send + Sync + 'static { ... }
```

conflicts with the generic impls from `#[resolvable]`:

```rust
impl<B> ResolveFrom<B> for Validator where Builtins: ResolveFrom<B> { ... }
```

At `B = Validator`, the compiler sees two potentially overlapping impls. It can't prove the where-clauses are mutually exclusive, so it rejects the code. **Resolution**: remove the blanket impl. Base types get their `ResolveFrom` impl from `resolver!()` instead.

## 4. API Surface

### Declaring a Derived Type

```rust
#[resolvable]
impl Client {
    fn new(validator: &Validator, builtins: &Builtins, telemetry: &Telemetry) -> Self {
        Self { validator: validator.clone(), builtins: builtins.clone(), telemetry: telemetry.clone() }
    }

    fn number(&self) -> i32 { ... }  // other methods preserved as-is
}
```

**Constraints on `new`**: must exist, all params must be `&Type`, return type must be `Self`, no `self` receiver.

### Creating a Resolver

```rust
let mut resolver = autoresolve::resolver!(
    builtins: Builtins,
    telemetry: Telemetry,
    request: Request,
);
let outbound = resolver.get::<OutboundClient>();
```

Each `name: Type` pair provides a root instance. The variable `name` must be in scope.

## 5. Generated Code

### `#[resolvable]` Output (for `Client` above)

The original `impl Client { ... }` block is preserved unchanged, plus:

```rust
impl<__AutoresolveBase> ::autoresolve::ResolveFrom<__AutoresolveBase> for Client
where
    __AutoresolveBase: Send + Sync + 'static,
    Validator: ::autoresolve::ResolveFrom<__AutoresolveBase>,
    Builtins: ::autoresolve::ResolveFrom<__AutoresolveBase>,
    Telemetry: ::autoresolve::ResolveFrom<__AutoresolveBase>,
{
    type Inputs = ::autoresolve::ResolutionDepsNode<
        Validator,
        ::autoresolve::ResolutionDepsNode<
            Builtins,
            ::autoresolve::ResolutionDepsNode<
                Telemetry,
                ::autoresolve::ResolutionDepsEnd,
            >,
        >,
    >;

    fn new(
        inputs: <Self::Inputs as ::autoresolve::ResolutionDeps<__AutoresolveBase>>::Resolved<'_>,
    ) -> Self {
        let ::autoresolve::ResolutionDepsNode(
            dep_0,
            ::autoresolve::ResolutionDepsNode(
                dep_1,
                ::autoresolve::ResolutionDepsNode(dep_2, ::autoresolve::ResolutionDepsEnd),
            ),
        ) = inputs;
        Client::new(dep_0, dep_1, dep_2)
    }
}
```

Key properties:
- **Generic over `__AutoresolveBase`** with **where-bounds on each dependency** — the compiler transitively verifies the full graph.
- **Fully qualified paths** (`::autoresolve::...`) — no import requirements at the use site.
- **Calls the inherent `Client::new()`** (not the trait method) — Rust resolves this unambiguously due to different parameter signatures.
- **Original impl block preserved** — all other methods are untouched.

### `resolver!()` Output

```rust
let mut resolver = {
    struct __AutoresolveBase;
    unsafe impl Send for __AutoresolveBase {}
    unsafe impl Sync for __AutoresolveBase {}

    impl ::autoresolve::ResolveFrom<__AutoresolveBase> for Builtins {
        type Inputs = ::autoresolve::ResolutionDepsEnd;
        fn new(_: ::autoresolve::ResolutionDepsEnd) -> Self {
            unreachable!("base types are pre-inserted into the resolver")
        }
    }
    // ... same for Telemetry, Request ...

    let mut r = ::autoresolve::Resolver::<__AutoresolveBase>::new_empty();
    r.insert(builtins);
    r.insert(telemetry);
    r.insert(request);
    r
};
```

Key properties:
- **Block-local `__AutoresolveBase`** — each call site gets a unique anonymous type. Prevents cross-resolver confusion.
- **Orphan rule compliance** — `impl ResolveFrom<__AutoresolveBase> for Builtins` is legal even if `Builtins` comes from a foreign crate. Rust's orphan rule for `impl ForeignTrait<T1..Tn> for T0` requires at least one of `T0..Tn` to be a local type with no uncovered type parameters before it. `__AutoresolveBase` is a concrete local type (defined at the macro expansion site in the user's crate), satisfying the rule at `T1` regardless of whether `T0` is local or foreign.
- **`unreachable!()` in base `new()`** — safe because `Resolver::get()` checks `contains()` before `new()`, and base types are pre-inserted.
- **`unsafe impl Send/Sync`** — the phantom type is a ZST used only as a type parameter; it's never instantiated or shared.

## 6. Compile-Time Error Examples

### Missing Dependency

```rust
let mut resolver = autoresolve::resolver!(builtins: Builtins, telemetry: Telemetry);
//                                        ^^^^^^^^ no `request: Request`
resolver.get::<OutboundClient>();
```

```
error[E0277]: the trait bound `Request: ResolveFrom<__AutoresolveBase>` is not satisfied
  = note: required for `CorrelationVector` to implement `ResolveFrom<__AutoresolveBase>`
  = note: required for `OutboundClient` to implement `ResolveFrom<__AutoresolveBase>`
```

The chain of `required for` notes shows the full transitive path.

### Dependency Cycle

```rust
#[resolvable]
impl A { fn new(b: &B) -> Self { ... } }

#[resolvable]
impl B { fn new(a: &A) -> Self { ... } }
```

```
error[E0275]: overflow evaluating the requirement `A: ResolveFrom<__AutoresolveBase>`
  = note: required for `B` to implement `ResolveFrom<__AutoresolveBase>`
  = note: required for `A` to implement `ResolveFrom<__AutoresolveBase>` ...
```

## 7. Runtime Behavior

When `resolver.get::<OutboundClient>()` is called:

1. `OutboundClient` not in TypeMap → resolve its `Inputs`.
2. `ensure()` walks the dependency list depth-first — `ensure::<CorrelationVector>()` → `ensure::<Request>()` → cached (base) → construct CV → insert, etc.
3. `get_private()` retrieves `&CorrelationVector`, `&Client`, `&Builtins` from TypeMap.
4. `OutboundClient::new(cv, client, builtins)` constructs the value → inserted → returned.

**Caching**: each type is constructed at most once. `TypeMap::contains()` in `get()` ensures subsequent calls return the cached reference. All consumers of a dependency share the same instance.

## 8. Key Decisions

| Decision | Rationale |
|----------|-----------|
| Remove blanket `impl<T> ResolveFrom<T> for T` | Coherence conflict with generic `#[resolvable]` impls |
| `#[resolvable]` on `impl` blocks, not `#[derive]` on structs | More flexible — user controls which constructor is used |
| Generic derived impls (`impl<B> ... where deps: ResolveFrom<B>`) | Compiler's trait solver does transitive checking for free |
| Block-local phantom `__AutoresolveBase` | Unique per call site, avoids coherence, prevents cross-resolver confusion |
| `resolver!` as `macro_rules!` (not proc macro) | Pattern is expressible in declarative macros, no extra crate needed |
| Two-crate macro split (`_macros` + `_macros_impl`) | Matches workspace convention, enables unit-testing with `insta` |

## 9. Scope

**In scope**: `#[resolvable]`, `resolver!()`, `Resolver::new_empty()`/`insert()`, blanket impl removal, test updates, compile-fail tests (`trybuild`), snapshot tests (`insta`).

**Out of scope**: `#[derive(Resolvable)]`, async resolution, scoped resolvers, cleanup of experimental `Something`/`Resolvable`/`RC`/`RN` types.

## 10. Open Questions

1. **Should `resolver!` support overriding a `#[resolvable]` type with a pre-constructed instance?** Useful for testing (provide a mock). Would require care to avoid coherence between the concrete base-impl and the generic resolvable-impl. Recommendation: defer.
2. **Should base types be required to be `Clone`?** No — they're moved into the resolver via `insert()`, consumers get `&T` references. Clone is only needed if the consuming type clones from the reference in its own constructor.
