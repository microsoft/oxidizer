<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Thread Aware Logo" width="96">

# Thread Aware

[![crate.io](https://img.shields.io/crates/v/thread_aware.svg)](https://crates.io/crates/thread_aware)
[![docs.rs](https://docs.rs/thread_aware/badge.svg)](https://docs.rs/thread_aware)
[![MSRV](https://img.shields.io/crates/msrv/thread_aware)](https://crates.io/crates/thread_aware)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Essential building blocks for thread-per-core libraries.

## Crate features

* The **`std` Cargo feature** *(enabled by default)* enables the strategy-partitioned `Arc` and
  [`ThreadBuilder`][__link0].
* **`derive`** *(default)* re-exports the `#[derive(ThreadAware)]` macro.
* Disable default features for `#![no_std]` environments. The core thread vocabulary,
  closures, and wrappers remain available.
  Enable `derive` explicitly if the derive macro is needed.

`no_std` environments require `alloc` and pointer-width atomics.

This crate re-exports every type from [`thread_aware_core`][__link1], which is the authoritative stable
relocation contract. It adds derive support, closures, wrappers, runtime thread construction,
and strategy-partitioned shared state.

## Theory of Operation

At a high level, this crate enables thread migrations of state via the [`ThreadAware`][__link2] trait:

* Runtimes (and similar) can use it to inform types that they were just moved across a thread or NUMA boundary.
* The authors of said types can then act on this information to implement performance optimizations. Such optimizations
  might include re-allocating memory in a new NUMA region, connecting to a thread-local I/O scheduler,
  or detaching from shared, possibly contended memory with the previous thread.

Similar to [`Clone`][__link3], there are no exact semantic prescriptions of how types should behave on relocation.
They might continue to share some state (e.g., a common cache) or fully detach from it for performance reasons.
The primary goal is performance, so types should aim to minimize contention on synchronization primitives
and cross-NUMA memory access. Like `Clone`, the relocation itself should be mostly transparent and predictable
to users.

### Implementing [`ThreadAware`][__link4], and `Arc<T, PerThread>`

In most cases [`ThreadAware`][__link5] should be implemented via the provided derive macro.
As thread-awareness of a type usually involves letting all contained fields know of an ongoing
relocation, the derive macro does just that. A default impl is provided for many `std` types,
so the macro should ‘just work’ on most compounds of built-ins.

External crates might often not implement [`ThreadAware`][__link6]. In many of these cases using our
[`thread_aware::Arc`][__link7] offers a convenient solution when the `std` feature is enabled: it
combines an upstream [`alloc::sync::Arc`][__link8] with a relocation [`Strategy`][__link9], and
implements [`ThreadAware`][__link10] for it. For
example, while an `Arc<Foo, PerProcess>` effectively acts as vanilla `Arc`, an
`Arc<Foo, PerThread>` ensures a separate `Foo` is available any time the types moves a core boundary.

### Relation to [`Send`][__link11]

[`ThreadAware`][__link12] requires [`Send`][__link13] as a supertrait. Types are first sent to another thread,
then the [`ThreadAware`][__link14] relocation notification is invoked.

### Thread vs. Core Semantics

As this library is primarily intended for use in thread-per-core runtimes,
we use the terms ‘thread’ and ‘core’ interchangeably. The assumption is that items
primarily relocate between different threads, where each thread is pinned to a different CPU core.
Should a runtime utilize more than one thread per core (e.g., for internal I/O) user code should
be able to observe this fact.

### [`ThreadAware`][__link15] vs. [`Unaware`][__link16]

Sometimes you might need to move inert types as-is, essentially bypassing all
thread-aware handling. These might be foreign types that carry no allocation, do
no I/O, or otherwise do not require any thread-specific handling.

[`Unaware`][__link17] can be used to encapsulate such types, a wrapper that itself implements [`ThreadAware`][__link18], but
otherwise does not react to it. You can think of it as a `MoveAsIs<T>`. However, it was
deliberately named `Unaware` to signal that only types which are genuinely unaware of their
thread relocations (i.e., don’t impl [`ThreadAware`][__link19]) should be wrapped in such.

Wrapping types that implement the trait is discouraged, as it will prevent them from properly
relocating and might have an impact on their performance, but not correctness, see below.

### Performance vs. Correctness

It is important to note that [`ThreadAware`][__link20] is a cooperative performance optimization and contention avoidance
primitive, not a guarantee of behavior for either the caller or callee. In other words, callers and runtimes must
continue to operate correctly if the trait is invoked incorrectly.

In particular, [`ThreadAware`][__link21] may not always be invoked when a type leaves the current thread.
While runtimes should reduce the incidence of that through their API design, it may nonetheless
happen via [`std::thread::spawn`][__link22] and other means. In these cases types should still function
correctly, although they might experience degraded performance through contention of now-shared
resources.

### Provided Implementations

[`thread_aware_core`][__link23] implements [`ThreadAware`][__link24] for core, alloc, and standard library types.
Implementations for third-party types live with those types once the stable trait can be adopted
natively. Until then, inert foreign values can be wrapped in [`Unaware`][__link25].

## Features

* The **`std` Cargo feature** *(enabled by default)* enables the strategy-partitioned `Arc` and
  [`ThreadBuilder`][__link26]. Disable it for `#![no_std]` environments; the crate
  then requires `alloc` and pointer-width atomics.
* **`derive`** *(default)*: Re-exports the `#[derive(ThreadAware)]` macro from the companion
  `thread_aware_macros` crate. Disable to avoid pulling in proc-macro code in minimal
  environments. For derive support without `std`, use
  `default-features = false, features = ["derive"]`.

## Examples

### Using the [`ThreadAware` derive macro][__link27]

When the `derive` feature (enabled by default) is active you can simply
use the [`ThreadAware` derive macro][__link28] instead of writing the
implementation manually.

```rust
use thread_aware::ThreadAware;

#[derive(Debug, Clone, ThreadAware)]
struct Point {
    x: i32,
    y: i32,
}
```

### Enabling [`ThreadAware`][__link29] via `Arc<T, S>`

With the `std` feature, types containing fields not [`ThreadAware`][__link30] can use [`Arc`][__link31] to specify a
strategy and wrap them in an [`Arc`][__link32] that implements the trait.

```rust
use thread_aware::{Arc, PerThread, ThreadAware};

#[derive(Debug, Clone, ThreadAware)]
struct Service {
    name: String,
    client: Arc<Client, PerThread>,
}

impl Service {
    fn new() -> Self {
        Self {
            name: "MyService".to_string(),
            client: Arc::new(|| Client::default()),
        }
    }
}
```


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/thread_aware">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQb5FWN1iQpmM8bLjOmKxdzwkcbBtJ9df6SOPobwpMVDTp1lk9hZIKCbHRocmVhZF9hd2FyZWYwLjExLjCCc3RocmVhZF9hd2FyZV9tYWNyb3NmMC4xMS4w
 [__link0]: https://docs.rs/thread_aware/0.11.0/thread_aware/?search=thread::ThreadBuilder
 [__link1]: https://docs.rs/thread_aware_core
 [__link10]: https://docs.rs/thread_aware_macros/0.11.0/thread_aware_macros/?search=ThreadAware
 [__link11]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link12]: https://docs.rs/thread_aware_macros/0.11.0/thread_aware_macros/?search=ThreadAware
 [__link13]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link14]: https://docs.rs/thread_aware_macros/0.11.0/thread_aware_macros/?search=ThreadAware
 [__link15]: https://docs.rs/thread_aware_macros/0.11.0/thread_aware_macros/?search=ThreadAware
 [__link16]: https://docs.rs/thread_aware/0.11.0/thread_aware/?search=Unaware
 [__link17]: https://docs.rs/thread_aware/0.11.0/thread_aware/?search=Unaware
 [__link18]: https://docs.rs/thread_aware_macros/0.11.0/thread_aware_macros/?search=ThreadAware
 [__link19]: https://docs.rs/thread_aware_macros/0.11.0/thread_aware_macros/?search=ThreadAware
 [__link2]: https://docs.rs/thread_aware_macros/0.11.0/thread_aware_macros/?search=ThreadAware
 [__link20]: https://docs.rs/thread_aware_macros/0.11.0/thread_aware_macros/?search=ThreadAware
 [__link21]: https://docs.rs/thread_aware_macros/0.11.0/thread_aware_macros/?search=ThreadAware
 [__link22]: https://doc.rust-lang.org/stable/std/?search=thread::spawn
 [__link23]: https://docs.rs/thread_aware_core
 [__link24]: https://docs.rs/thread_aware_macros/0.11.0/thread_aware_macros/?search=ThreadAware
 [__link25]: https://docs.rs/thread_aware/0.11.0/thread_aware/?search=Unaware
 [__link26]: https://docs.rs/thread_aware/0.11.0/thread_aware/?search=thread::ThreadBuilder
 [__link27]: https://docs.rs/thread_aware_macros/0.11.0/thread_aware_macros/?search=ThreadAware
 [__link28]: https://docs.rs/thread_aware_macros/0.11.0/thread_aware_macros/?search=ThreadAware
 [__link29]: https://docs.rs/thread_aware_macros/0.11.0/thread_aware_macros/?search=ThreadAware
 [__link3]: https://doc.rust-lang.org/stable/std/clone/trait.Clone.html
 [__link30]: https://docs.rs/thread_aware_macros/0.11.0/thread_aware_macros/?search=ThreadAware
 [__link31]: https://docs.rs/thread_aware/0.11.0/thread_aware/?search=Arc
 [__link32]: https://docs.rs/thread_aware/0.11.0/thread_aware/?search=Arc
 [__link4]: https://docs.rs/thread_aware_macros/0.11.0/thread_aware_macros/?search=ThreadAware
 [__link5]: https://docs.rs/thread_aware_macros/0.11.0/thread_aware_macros/?search=ThreadAware
 [__link6]: https://docs.rs/thread_aware_macros/0.11.0/thread_aware_macros/?search=ThreadAware
 [__link7]: https://docs.rs/thread_aware/0.11.0/thread_aware/?search=Arc
 [__link8]: https://doc.rust-lang.org/stable/alloc/?search=sync::Arc
 [__link9]: https://docs.rs/thread_aware/0.11.0/thread_aware/?search=storage::Strategy
