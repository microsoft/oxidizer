# Thread Aware Core Design

## Purpose

`thread_aware_core` provides core types for values that adapt when a runtime moves them between threads. Other crates can use these types in public APIs.

The stable public boundary contains:

- `ThreadAware`: The trait for values that adapt after a move.
- `Thread`: The runtime, OS thread, and nearest memory for a value.
- `Owner`: The identity of the runtime that created a `Thread`.
- `NumaNode`: The identity of a non-uniform memory access (NUMA) node.

The `thread_aware` crate provides derive macros, closure adapters, policy wrappers, and runtime construction. The `performables` crate provides strategy-partitioned shared state. Those utilities can change before version 1.0. This crate contains only the types that independent libraries must share.

Implementations for `core`, `alloc`, and `std` types are part of the shared API. Implementations for third-party types are outside this crate.

The doc-hidden `__private` module is publicly reachable for runtime integration. Its versioned constructors are unsupported implementation details and are not part of the stable boundary.

## Why this crate is separate

Rust treats the same traits from different versions of the same crate as different types. Independent libraries can use one relocation contract only if they depend on the same `ThreadAware` definition.

Public APIs name the types from this crate. An incompatible release would require a coordinated migration before old and new libraries could interoperate again. A small API, no normal dependencies, and a deliberate minimum supported Rust version (MSRV) policy reduce that migration risk.

The separate `thread_aware` crate owns convenience APIs. It can change those APIs without changing the core contract.

## Scope

`Owner` describes a runtime. `NumaNode` describes a hardware memory region. A thread id describes a live OS thread.

These ids have different lifetimes and guarantees. An `Owner` is meaningful while its runtime is live. A thread id identifies its OS thread. A `NumaNode` can be shared across runtimes only if they use the same node numbering.

Runtime integration code creates `Owner` values. Each new owner has a distinct identity. Caller configuration does not select that identity.

`ThreadAware` requires `Send`. A type that is not `Send`, such as a type that contains `Rc`, cannot implement the trait.

## Principles

### Relocation affects performance, not correctness

A value must remain correct if `relocate` is never called, called more than once, or called with an unfamiliar `Thread`. This rule lets a runtime call the method when useful. It also lets an implementation keep usable old state when ideal adaptation is not available.

The runtime controls call frequency. Implementations must keep calls with unchanged relevant coordinates cheap.

### The destination thread runs relocation

The runtime first moves or schedules the value. The destination thread then calls `relocate`.

The `destination` coordinate describes the thread that runs the callback. A coordinator must not call the method for another thread. This order lets an implementation release or create local state through the destination thread's allocator and NUMA locality.

### Shared contracts define the stable API

Add an item only if independent crates must agree on it. Helpers that make implementation easier belong in `thread_aware`.

### Standard thread identifiers

`Thread` uses `std::thread::ThreadId`. A separate id type requires conversion and another permanent contract.

This choice means that `__private::v1::new_thread` and `Thread::id` need the `std` feature. Without default features, a crate can still implement `ThreadAware` and read `Owner` and `NumaNode`. It cannot construct a `Thread`.

A `no_std` build needs `alloc` and pointer-width atomics. The atomics provide unique `Owner` identities.

### No normal dependencies

The manifest has one test-only development dependency. Using this crate adds no other production package.

### Documented runtime obligations

The crate cannot make runtimes use the same NUMA numbering. It also cannot prevent a `relocate` implementation from blocking. These are documented requirements because enforcement requires runtime state and coordination.

## Additive evolution

Hardware can add locality levels that `Thread` does not model. A future version can add a private coordinate without changing existing construction functions.

### Opaque identifiers

`NumaNode` does not expose its integer or implement `From<integer>`. Runtime code uses a typed, versioned constructor. `Owner` accepts no caller-provided identity.

Multiple integer conversions can change how unsuffixed literals resolve. Replacing a conversion can also break typed callers. `cargo semver-checks` does not report every such change.

A typed constructor avoids this problem. A future wider or fallible form can use a new function name. The private stored width can change without changing the public contract.

### Private `Thread` fields

Accessors read each coordinate. A new private field does not change existing readers. Derived `Clone`, `Debug`, `PartialEq`, `Eq`, and `Hash` include the new field.

A field can still remove an auto trait such as `Send`, `Sync`, or unwind safety. Static assertions make such a change fail the build.

### Default values for new coordinates

A new coordinate needs a value that means unknown or not pinned. Existing constructors keep their signatures and use this default. A new constructor lets callers provide the coordinate.

The default value must not collide with a value from an existing constructor. The implementation can widen private storage or make a new constructor fallible to reserve the value.

Existing construction paths must use the same default. Values created through those paths then keep their previous equality behavior.

Adding a coordinate follows these steps:

1. Add the private field and accessor.
2. Add the reserved default to the coordinate type.
3. Add a constructor that accepts the new coordinate.

### Dyn-compatible trait additions

`dyn ThreadAware` is part of the stable API. A new method needs a default implementation. It also must avoid generic parameters, `impl Trait`, and `Self` outside the receiver, unless it uses `where Self: Sized`.

A static assertion makes a loss of dyn compatibility fail the build.

## Prohibited compatibility changes

The compatibility contract prohibits these changes:

- Remove a coordinate.
- Change an existing constructor signature.
- Add a `From<integer>` implementation.
- Change the meaning of an existing id.
- Add a method that breaks dyn compatibility.

These changes require a separate incompatible contract instead of an additive update.

## Related documents

See [STABILIZATION.md](./STABILIZATION.md) for the items in the stable boundary.
