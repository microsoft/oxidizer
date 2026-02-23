<div align="center">
 <img src="./logo.png" alt="Thread Aware Logo" width="96">

# Thread Aware

[![crate.io](https://img.shields.io/crates/v/thread_aware.svg)](https://crates.io/crates/thread_aware)
[![docs.rs](https://docs.rs/thread_aware/badge.svg)](https://docs.rs/thread_aware)
[![MSRV](https://img.shields.io/crates/msrv/thread_aware)](https://crates.io/crates/thread_aware)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Essential building blocks for thread-per-core libraries.

This crate allows you to express migrations between NUMA nodes, threads, or specific CPU cores.
It can serve as a foundation for building components and runtimes that operate across multiple
memory affinities.

## Theory of Operation

At a high level, this crate enables thread migrations of state via the [`ThreadAware`][__link0] trait:

* Runtimes (and similar) can use it to inform types that they were just moved across a thread or NUMA boundary.
* The authors of said types can then act on this information to implement performance optimizations. Such optimizations
  might include re-allocating memory in a new NUMA region, connecting to a thread-local I/O scheduler,
  or detaching from shared, possibly contended memory with the previous thread.

Similar to [`Clone`][__link1], there are no exact semantic prescriptions of how types should behave on relocation.
They might continue to share some state (e.g., a common cache) or fully detach from it for performance reasons.
The primary goal is performance, so types should aim to minimize contention on synchronization primitives
and cross-NUMA memory access. Like `Clone`, the relocation itself should be mostly transparent and predictable
to users.

### Implementing [`ThreadAware`][__link2], and `Arc<T, PerCore>`

In most cases [`ThreadAware`][__link3] should be implemented via the provided derive macro.
As thread-awareness of a type usually involves letting all contained fields know of an ongoing
relocation, the derive macro does just that. A default impl is provided for many `std` types,
so the macro should ‘just work’ on most compounds of built-ins.

External crates might often not implement [`ThreadAware`][__link4]. In many of these cases using our
[`thread_aware::Arc`][__link5] offers a convenient solution: It combines an upstream
[`std::sync::Arc`][__link6] with a relocation [`Strategy`][__link7], and implements [`ThreadAware`][__link8] for it. For
example, while an `Arc<Foo, PerProcess>` effectively acts as vanilla `Arc`, an
`Arc<Foo, PerCore>` ensures a separate `Foo` is available any time the types moves a core boundary.

### Relation to [`Send`][__link9]

Although [`ThreadAware`][__link10] has no supertraits, any runtime invoking it will usually require the underlying type to
be [`Send`][__link11]. In these cases, type are first sent to another thread, then the [`ThreadAware`][__link12] relocation
notification is invoked.

### Thread vs. Core Semantics

As this library is primarily intended for use in thread-per-core runtimes,
we use the terms ‘thread’ and ‘core’ interchangeably. The assumption is that items
primarily relocate between different threads, where each thread is pinned to a different CPU core.
Should a runtime utilize more than one thread per core (e.g., for internal I/O) user code should
be able to observe this fact.

### [`ThreadAware`][__link13] vs. [`Unaware`][__link14]

Sometimes you might need to move inert types as-is, essentially bypassing all
thread-aware handling. These might be foreign types that carry no allocation, do
no I/O, or otherwise do not require any thread-specific handling.

[`Unaware`][__link15] can be used to encapsulate such types, a wrapper that itself implements [`ThreadAware`][__link16], but
otherwise does not react to it. You can think of it as a `MoveAsIs<T>`. However, it was
deliberately named `Unaware` to signal that only types which are genuinely unaware of their
thread relocations (i.e., don’t impl [`ThreadAware`][__link17]) should be wrapped in such.

Wrapping types that implement the trait is discouraged, as it will prevent them from properly
relocating and might have an impact on their performance, but not correctness, see below.

### Performance vs. Correctness

It is important to note that [`ThreadAware`][__link18] is a cooperative performance optimization and contention avoidance
primitive, not a guarantee of behavior for either the caller or callee. In other words, callers and runtimes must
continue to operate correctly if the trait is invoked incorrectly.

In particular, [`ThreadAware`][__link19] may not always be invoked when a type leaves the current thread.
While runtimes should reduce the incidence of that through their API design, it may nonetheless
happen via [`std::thread::spawn`][__link20] and other means. In these cases types should still function
correctly, although they might experience degraded performance through contention of now-shared
resources.

### Provided Implementations

[`ThreadAware`][__link21] is implemented for many standard library types, including primitive types, Vec,
String, Option, Result, tuples, etc. However, it’s explicitly not implemented for [`std::sync::Arc`][__link22]
as that type implies some level of cross-thread sharing and thus needs special attention when used
from types that implement [`ThreadAware`][__link23].

## Features

* **`derive`** *(default)*: Re-exports the `#[derive(ThreadAware)]` macro from the companion
  `thread_aware_macros` crate. Disable to avoid pulling in proc-macro code in minimal
  environments: `default-features = false`.
* **`threads`**: Enables features mainly used by async runtimes for OS interactions.

## Examples

### Deriving [`ThreadAware`][__link24]

When the `derive` feature (enabled by default) is active you can simply
derive [`ThreadAware`][__link25] instead of writing the implementation manually.

```rust
use thread_aware::ThreadAware;

#[derive(Debug, Clone, ThreadAware)]
struct Point {
    x: i32,
    y: i32,
}
```

### Enabling [`ThreadAware`][__link26] via `Arc<T, S>`

For types containing fields not [`ThreadAware`][__link27], you can use [`Arc`][__link28] to specify a
strategy, and wrap them in an [`Arc`][__link29] that implements the trait.

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
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/thread_aware">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEG5MT6aAp-ABuGy-pd26b7pShG0TsTpNCyjUHGwNfk8mMYEfpYWSCgmx0aHJlYWRfYXdhcmVlMC42LjKCc3RocmVhZF9hd2FyZV9tYWNyb3NlMC42LjE
 [__link0]: https://docs.rs/thread_aware_macros/0.6.1/thread_aware_macros/?search=ThreadAware
 [__link1]: https://doc.rust-lang.org/stable/std/clone/trait.Clone.html
 [__link10]: https://docs.rs/thread_aware_macros/0.6.1/thread_aware_macros/?search=ThreadAware
 [__link11]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link12]: https://docs.rs/thread_aware_macros/0.6.1/thread_aware_macros/?search=ThreadAware
 [__link13]: https://docs.rs/thread_aware_macros/0.6.1/thread_aware_macros/?search=ThreadAware
 [__link14]: https://docs.rs/thread_aware/0.6.2/thread_aware/?search=Unaware
 [__link15]: https://docs.rs/thread_aware/0.6.2/thread_aware/?search=Unaware
 [__link16]: https://docs.rs/thread_aware_macros/0.6.1/thread_aware_macros/?search=ThreadAware
 [__link17]: https://docs.rs/thread_aware_macros/0.6.1/thread_aware_macros/?search=ThreadAware
 [__link18]: https://docs.rs/thread_aware_macros/0.6.1/thread_aware_macros/?search=ThreadAware
 [__link19]: https://docs.rs/thread_aware_macros/0.6.1/thread_aware_macros/?search=ThreadAware
 [__link2]: https://docs.rs/thread_aware_macros/0.6.1/thread_aware_macros/?search=ThreadAware
 [__link20]: https://doc.rust-lang.org/stable/std/?search=thread::spawn
 [__link21]: https://docs.rs/thread_aware_macros/0.6.1/thread_aware_macros/?search=ThreadAware
 [__link22]: https://doc.rust-lang.org/stable/std/?search=sync::Arc
 [__link23]: https://docs.rs/thread_aware_macros/0.6.1/thread_aware_macros/?search=ThreadAware
 [__link24]: https://docs.rs/thread_aware_macros/0.6.1/thread_aware_macros/?search=ThreadAware
 [__link25]: https://docs.rs/thread_aware_macros/0.6.1/thread_aware_macros/?search=ThreadAware
 [__link26]: https://docs.rs/thread_aware_macros/0.6.1/thread_aware_macros/?search=ThreadAware
 [__link27]: https://docs.rs/thread_aware_macros/0.6.1/thread_aware_macros/?search=ThreadAware
 [__link28]: https://docs.rs/thread_aware/0.6.2/thread_aware/?search=Arc
 [__link29]: https://docs.rs/thread_aware/0.6.2/thread_aware/?search=Arc
 [__link3]: https://docs.rs/thread_aware_macros/0.6.1/thread_aware_macros/?search=ThreadAware
 [__link4]: https://docs.rs/thread_aware_macros/0.6.1/thread_aware_macros/?search=ThreadAware
 [__link5]: https://docs.rs/thread_aware/0.6.2/thread_aware/?search=Arc
 [__link6]: https://doc.rust-lang.org/stable/std/?search=sync::Arc
 [__link7]: https://docs.rs/thread_aware/0.6.2/thread_aware/?search=storage::Strategy
 [__link8]: https://docs.rs/thread_aware_macros/0.6.1/thread_aware_macros/?search=ThreadAware
 [__link9]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
