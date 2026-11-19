<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Thread Aware Core Logo" width="96">

# Thread Aware Core

[![crate.io](https://img.shields.io/crates/v/thread_aware_core.svg)](https://crates.io/crates/thread_aware_core)
[![docs.rs](https://docs.rs/thread_aware_core/badge.svg)](https://docs.rs/thread_aware_core)
[![MSRV](https://img.shields.io/crates/msrv/thread_aware_core)](https://crates.io/crates/thread_aware_core)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Core types for values that adapt after a move to another thread.

This crate provides the shared API for thread-aware libraries:

* [`ThreadAware`][__link0] is the trait for values that adapt after a move.
* [`Thread`][__link1] describes the runtime, OS thread, and nearest memory for a value.

[`Thread`][__link2] is a coordinate, not a handle. It owns no operating-system resource.
[`std::thread::Thread`][__link3] is a separate handle to a live OS thread. Use an alias if a module
needs both types.

## The `thread_aware` family

Use this crate for the core types that public APIs expose. It has no derive macros, closure
adapters, policy wrappers, or runtime integration.

The [`thread_aware`][__link4] crate provides those utilities. It includes a
[`#[derive(ThreadAware)]`][__link5] macro, closure adapters, wrappers for foreign types, and
runtime coordinate construction. Its [authoring guide][__link6] explains how to make a type
thread-aware. The `performables` crate provides strategy-partitioned shared state.

Depend on this crate directly if you only need the core types. It has no normal
dependencies. Without the default `std` feature, [`Thread`][__link7] keeps [`Owner`][__link8] and [`NumaNode`][__link9]
but has no thread id.

## Why relocation exists

A runtime can keep memory and resources close to each worker. A moved value can still refer to
memory or resources from its old worker. [`ThreadAware`][__link10] lets the value adapt this state.
The runtime moves the value, then calls [`relocate`][__link11] on its new thread.

## Library authors: implementing the trait

Library and application authors implement [`ThreadAware`][__link12]. They usually use the
[`#[derive(ThreadAware)]`][__link13] macro. A runtime constructs [`Thread`][__link14] values and calls
[`relocate`][__link15]. The method is a callback like [`Drop::drop`][__link16].

The derive macro is in [`thread_aware`][__link17]. A library can use that crate as an implementation
dependency while its public API uses types from this crate:

```rust
// An implementation dependency, not part of the public API.
use thread_aware::ThreadAware;

/// A codec with a scratch buffer that follows its nearest memory.
#[derive(ThreadAware)]
pub struct Encoder {
    scratch: Scratch,
    dictionary: Dictionary,
}
```

The derive forwards `relocate` to `scratch` and `dictionary`. Callers of `Encoder` do not need
to name [`thread_aware`][__link18].

## Runtime authors: driving relocation

Runtime authors create [`Thread`][__link19] values for their workers. After a runtime moves a value, the
destination thread calls [`relocate`][__link20].

`destination` describes the thread that runs the callback. It does not request work on another
thread. This order lets the callback release or create state with the destination thread’s
memory locality.

## Performance, not correctness

Relocation is an optimization. A value must remain correct if the runtime never calls
`relocate`, calls it more than once, or gives it equal coordinates. A missed call can reduce
performance, but it must not cause a panic, deadlock, or wrong result.

The runtime controls call frequency. Implementations must keep calls with unchanged relevant
coordinates cheap. They must also avoid contended synchronization and long blocking work.

## What the ids mean

* A thread id identifies a live OS thread.
* [`NumaNode`][__link21] identifies nearby memory. Runtimes can compare nodes only if they use the same
  numbering.
* [`Owner`][__link22] identifies the runtime that owns a [`Thread`][__link23].

These ids are opaque and do not have to be consecutive. Use only the coordinate that the state
needs. Use a map instead of an indexed array for keyed state.

## Relation to `Send`

[`ThreadAware`][__link24] requires [`Send`][__link25]. The runtime first sends the value to its destination thread.
It then tells the value where it arrived. [`Send`][__link26] makes the move safe. [`ThreadAware`][__link27] adds
only the adaptation callback.

## Provided implementations

Values without thread-local state use an empty implementation. Containers forward relocation
to their values. Map keys stay unchanged.

General references, sets, `Cow`, and `Arc` have no implementation. Their relocation policy is
ambiguous or can break their invariants. The exception is `&'static str`, which cannot dangle
and has no referent state to relocate. [`thread_aware`][__link28] provides wrappers for explicit policies.
The `performables` crate provides a strategy-partitioned [`Arc`][__link29].

## Features

* `std` (default): Adds runtime construction support and [`Thread::id`][__link30]. It also implements
  [`ThreadAware`][__link31] for `HashMap`, `Path`, `PathBuf`, `SocketAddr`, `SocketAddrV4`, and
  `SocketAddrV6`.

Without `std`, the crate needs `alloc` and pointer-width atomics.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/thread_aware_core">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbrnOdj5SUkXAb8NyCbnmNwPwbGIl3BbkoElwbABFoXGU-0wxhZIGCcXRocmVhZF9hd2FyZV9jb3JlZTAuMS4z
 [__link0]: https://docs.rs/thread_aware_core/0.1.3/thread_aware_core/?search=ThreadAware
 [__link1]: https://docs.rs/thread_aware_core/0.1.3/thread_aware_core/?search=Thread
 [__link10]: https://docs.rs/thread_aware_core/0.1.3/thread_aware_core/?search=ThreadAware
 [__link11]: https://docs.rs/thread_aware_core/0.1.3/thread_aware_core/?search=ThreadAware::relocate
 [__link12]: https://docs.rs/thread_aware_core/0.1.3/thread_aware_core/?search=ThreadAware
 [__link13]: https://docs.rs/thread_aware/latest/thread_aware/derive.ThreadAware.html
 [__link14]: https://docs.rs/thread_aware_core/0.1.3/thread_aware_core/?search=Thread
 [__link15]: https://docs.rs/thread_aware_core/0.1.3/thread_aware_core/?search=ThreadAware::relocate
 [__link16]: https://doc.rust-lang.org/stable/std/?search=ops::Drop::drop
 [__link17]: https://docs.rs/thread_aware
 [__link18]: https://docs.rs/thread_aware
 [__link19]: https://docs.rs/thread_aware_core/0.1.3/thread_aware_core/?search=Thread
 [__link2]: https://docs.rs/thread_aware_core/0.1.3/thread_aware_core/?search=Thread
 [__link20]: https://docs.rs/thread_aware_core/0.1.3/thread_aware_core/?search=ThreadAware::relocate
 [__link21]: https://docs.rs/thread_aware_core/0.1.3/thread_aware_core/?search=NumaNode
 [__link22]: https://docs.rs/thread_aware_core/0.1.3/thread_aware_core/?search=Owner
 [__link23]: https://docs.rs/thread_aware_core/0.1.3/thread_aware_core/?search=Thread
 [__link24]: https://docs.rs/thread_aware_core/0.1.3/thread_aware_core/?search=ThreadAware
 [__link25]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link26]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link27]: https://docs.rs/thread_aware_core/0.1.3/thread_aware_core/?search=ThreadAware
 [__link28]: https://docs.rs/thread_aware
 [__link29]: https://docs.rs/performables/latest/performables/arc/struct.Arc.html
 [__link3]: https://doc.rust-lang.org/stable/std/?search=thread::Thread
 [__link30]: https://docs.rs/thread_aware_core/0.1.3/thread_aware_core/?search=Thread::id
 [__link31]: https://docs.rs/thread_aware_core/0.1.3/thread_aware_core/?search=ThreadAware
 [__link4]: https://docs.rs/thread_aware
 [__link5]: https://docs.rs/thread_aware/latest/thread_aware/derive.ThreadAware.html
 [__link6]: https://docs.rs/thread_aware/latest/thread_aware/documentation/index.html
 [__link7]: https://docs.rs/thread_aware_core/0.1.3/thread_aware_core/?search=Thread
 [__link8]: https://docs.rs/thread_aware_core/0.1.3/thread_aware_core/?search=Owner
 [__link9]: https://docs.rs/thread_aware_core/0.1.3/thread_aware_core/?search=NumaNode
