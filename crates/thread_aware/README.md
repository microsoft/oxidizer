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

* The **`std` Cargo feature** *(enabled by default)* enables standard-library
  implementations, the per-affinity `Arc`, and hosted-only closure support.
* **`derive`** *(default)* re-exports the `#[derive(ThreadAware)]` macro.
* **`threads`** enables the `registry` module and implies `std`.
* Disable default features for `#![no_std]` environments. The [`ThreadAware`][__link0] trait, affinity
  identifiers, closures, wrappers, and implementations for `alloc` types remain available.
  Enable `derive` explicitly if the derive macro is needed.

`no_std` environments require `alloc` and pointer-width atomics.

This crate allows you to express migrations between NUMA nodes, threads, or specific CPU cores.
It can serve as a foundation for building components and runtimes that operate across multiple
memory affinities.

## Theory of Operation

At a high level, this crate enables thread migrations of state via the [`ThreadAware`][__link1] trait:

* Runtimes (and similar) can use it to inform types that they were just moved across a thread or NUMA boundary.
* The authors of said types can then act on this information to implement performance optimizations. Such optimizations
  might include re-allocating memory in a new NUMA region, connecting to a thread-local I/O scheduler,
  or detaching from shared, possibly contended memory with the previous thread.

Similar to [`Clone`][__link2], there are no exact semantic prescriptions of how types should behave on relocation.
They might continue to share some state (e.g., a common cache) or fully detach from it for performance reasons.
The primary goal is performance, so types should aim to minimize contention on synchronization primitives
and cross-NUMA memory access. Like `Clone`, the relocation itself should be mostly transparent and predictable
to users.

### Implementing [`ThreadAware`][__link3], and `Arc<T, PerCore>`

In most cases [`ThreadAware`][__link4] should be implemented via the provided derive macro.
As thread-awareness of a type usually involves letting all contained fields know of an ongoing
relocation, the derive macro does just that. A default impl is provided for many `std` types,
so the macro should ‘just work’ on most compounds of built-ins.

External crates might often not implement [`ThreadAware`][__link5]. In many of these cases using our
[`thread_aware::Arc`][__link6] offers a convenient solution when the `std` feature is enabled: it
combines an upstream [`alloc::sync::Arc`][__link7] with a relocation [`Strategy`][__link8], and
implements [`ThreadAware`][__link9] for it. For
example, while an `Arc<Foo, PerProcess>` effectively acts as vanilla `Arc`, an
`Arc<Foo, PerCore>` ensures a separate `Foo` is available any time the types moves a core boundary.

### Relation to [`Send`][__link10]

[`ThreadAware`][__link11] requires [`Send`][__link12] as a supertrait. Types are first sent to another thread,
then the [`ThreadAware`][__link13] relocation notification is invoked.

### Thread vs. Core Semantics

As this library is primarily intended for use in thread-per-core runtimes,
we use the terms ‘thread’ and ‘core’ interchangeably. The assumption is that items
primarily relocate between different threads, where each thread is pinned to a different CPU core.
Should a runtime utilize more than one thread per core (e.g., for internal I/O) user code should
be able to observe this fact.

### [`ThreadAware`][__link14] vs. [`Unaware`][__link15]

Sometimes you might need to move inert types as-is, essentially bypassing all
thread-aware handling. These might be foreign types that carry no allocation, do
no I/O, or otherwise do not require any thread-specific handling.

[`Unaware`][__link16] can be used to encapsulate such types, a wrapper that itself implements [`ThreadAware`][__link17], but
otherwise does not react to it. You can think of it as a `MoveAsIs<T>`. However, it was
deliberately named `Unaware` to signal that only types which are genuinely unaware of their
thread relocations (i.e., don’t impl [`ThreadAware`][__link18]) should be wrapped in such.

Wrapping types that implement the trait is discouraged, as it will prevent them from properly
relocating and might have an impact on their performance, but not correctness, see below.

### Performance vs. Correctness

It is important to note that [`ThreadAware`][__link19] is a cooperative performance optimization and contention avoidance
primitive, not a guarantee of behavior for either the caller or callee. In other words, callers and runtimes must
continue to operate correctly if the trait is invoked incorrectly.

In particular, [`ThreadAware`][__link20] may not always be invoked when a type leaves the current thread.
While runtimes should reduce the incidence of that through their API design, it may nonetheless
happen via [`std::thread::spawn`][__link21] and other means. In these cases types should still function
correctly, although they might experience degraded performance through contention of now-shared
resources.

### Provided Implementations

[`ThreadAware`][__link22] is implemented for many standard library types, including primitive types, Vec,
String, Option, Result, tuples, etc. However, it’s explicitly not implemented for [`alloc::sync::Arc`][__link23]
as that type implies some level of cross-thread sharing and thus needs special attention when used
from types that implement [`ThreadAware`][__link24].

## Features

* The **`std` Cargo feature** *(enabled by default)* enables standard-library
  implementations, the per-affinity [`Arc`][__link25], and hosted-only closure support. Disable it
  for `#![no_std]` environments; the crate then requires `alloc` and pointer-width atomics.
* **`derive`** *(default)*: Re-exports the `#[derive(ThreadAware)]` macro from the companion
  `thread_aware_macros` crate. Disable to avoid pulling in proc-macro code in minimal
  environments. For derive support without `std`, use
  `default-features = false, features = ["derive"]`.
* **`threads`**: Enables features mainly used by async runtimes for OS interactions and implies
  `std`.

## Examples

### Using the [`ThreadAware` derive macro][__link26]

When the `derive` feature (enabled by default) is active you can simply
use the [`ThreadAware` derive macro][__link27] instead of writing the
implementation manually.

```rust
use thread_aware::ThreadAware;

#[derive(Debug, Clone, ThreadAware)]
struct Point {
    x: i32,
    y: i32,
}
```

### Enabling [`ThreadAware`][__link28] via `Arc<T, S>`

With the `std` feature, types containing fields not [`ThreadAware`][__link29] can use [`Arc`][__link30] to specify a
strategy and wrap them in an [`Arc`][__link31] that implements the trait.

```rust
use thread_aware::{Arc, PerCore, ThreadAware};

#[derive(Debug, Clone, ThreadAware)]
struct Service {
    name: String,
    client: Arc<Client, PerCore>,
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

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQb4jiqJagoWNAb9mVDMKyuDlwbfHfDdrUw4B4b9hgh4T92mR1hZIOCbHRocmVhZF9hd2FyZWUwLjkuMIJxdGhyZWFkX2F3YXJlX2NvcmVlMC4xLjCCc3RocmVhZF9hd2FyZV9tYWNyb3NlMC43LjU
 [__link0]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link1]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link10]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link11]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link12]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link13]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link14]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link15]: https://docs.rs/thread_aware/0.9.0/thread_aware/?search=Unaware
 [__link16]: https://docs.rs/thread_aware/0.9.0/thread_aware/?search=Unaware
 [__link17]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link18]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link19]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link2]: https://doc.rust-lang.org/stable/std/clone/trait.Clone.html
 [__link20]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link21]: https://doc.rust-lang.org/stable/std/?search=thread::spawn
 [__link22]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link23]: https://doc.rust-lang.org/stable/alloc/?search=sync::Arc
 [__link24]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link25]: https://docs.rs/thread_aware/0.9.0/thread_aware/?search=Arc
 [__link26]: https://docs.rs/thread_aware_macros/0.7.5/thread_aware_macros/?search=ThreadAware
 [__link27]: https://docs.rs/thread_aware_macros/0.7.5/thread_aware_macros/?search=ThreadAware
 [__link28]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link29]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link3]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link30]: https://docs.rs/thread_aware/0.9.0/thread_aware/?search=Arc
 [__link31]: https://docs.rs/thread_aware/0.9.0/thread_aware/?search=Arc
 [__link4]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link5]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link6]: https://docs.rs/thread_aware/0.9.0/thread_aware/?search=Arc
 [__link7]: https://doc.rust-lang.org/stable/alloc/?search=sync::Arc
 [__link8]: https://docs.rs/thread_aware/0.9.0/thread_aware/?search=storage::Strategy
 [__link9]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
