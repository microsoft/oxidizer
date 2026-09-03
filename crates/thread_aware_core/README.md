<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Thread Aware Core Logo" width="96">

# Thread Aware Core

[![crate.io](https://img.shields.io/crates/v/thread_aware_core.svg)](https://crates.io/crates/thread_aware_core)
[![docs.rs](https://docs.rs/thread_aware_core/badge.svg)](https://docs.rs/thread_aware_core)
[![MSRV](https://img.shields.io/crates/msrv/thread_aware_core)](https://crates.io/crates/thread_aware_core)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Support for values that adapt when a runtime moves them to another thread.

This crate contains the small API shared by thread-aware libraries:

* [`ThreadAware`][__link0] is the trait for values that adapt after a move; its
  [`relocate`][__link1] callback reports the destination.
* [`Thread`][__link2] is the coordinate where a value now runs, composed of runtime, OS-thread, and
  nearest-memory identifiers.

[`Thread`][__link3] is a coordinate, not a handle: a runtime builds one to describe where a value
is running, and it owns no operating-system resource. It is unrelated to
[`std::thread::Thread`][__link4], which is a handle to a live OS thread. Naming both in one module
requires aliasing one of them.

## The `thread_aware` family

Downstream libraries need one small contract they can implement and expose in public APIs
without also adopting derive macros, policy wrappers, registries, or runtime integration.
Keeping that contract here lets unrelated libraries interoperate while the larger utility
surface evolves independently.

* **`thread_aware_core`** (this crate) — the vocabulary that two unrelated libraries must
  agree on before either can relocate a value defined by the other. It evolves
  conservatively, reducing how much public APIs couple to changes in the utility crate.
* **[`thread_aware`][__link5]** — the utilities that make relocation convenient: a
  [`#[derive(ThreadAware)]`][__link6] macro, wrappers for foreign types, a per-core
  [`Arc`][__link7], containers and registries. Free to evolve, and not meant to appear in a
  public API.

Depend on this crate directly when all you need is the trait. It has no normal dependencies
and works without `std`: with default features turned off, [`Thread`][__link8] loses its thread id
component and keeps [`Owner`][__link9] and [`NumaNode`][__link10].

## Why relocation exists

Thread-per-core and non-uniform memory access (NUMA)-aware runtimes are fast because each
worker keeps to itself: it uses memory close to its own thread, talks to its own I/O
driver, and does not synchronize with other workers. When a value moves to another
worker, what used to be close by is now in the wrong place: a cache line shared between
threads, memory in a distant region, a handle to another thread’s driver.

[`ThreadAware`][__link11] lets that state repair itself. The runtime moves the value, then calls
[`relocate`][__link12] to report where it now lives. Relocation has two
sides, and most code sits on only one of them.

## Library authors: implementing the trait

**Library and application authors** implement [`ThreadAware`][__link13], usually through the
[`#[derive(ThreadAware)]`][__link14] macro. They never call
[`relocate`][__link15] and never construct a [`Thread`][__link16]; the runtime does
both and then invokes the implementation. It is a callback, like [`Drop::drop`][__link17].

The derive lives in [`thread_aware`][__link18], so a library that wants it depends on that crate.
Only the trait and [`Thread`][__link19] cross the public boundary, and both come from here, so the
dependency stays an implementation detail:

```rust
// A build dependency, not part of what this library promises.
use thread_aware::ThreadAware;

/// A codec whose scratch buffer should follow the memory it is used from.
#[derive(ThreadAware)]
pub struct Encoder {
    scratch: Scratch,
    dictionary: Dictionary,
}
```

The derive writes the forwarding implementation, calling `relocate` on `scratch` and
`dictionary` in turn. Because a composed type forwards to its fields, one call at the top
reaches everything below it. Callers of `Encoder` never name [`thread_aware`][__link20].

## Runtime authors: driving relocation

**Runtime authors** create a list of [`Thread`][__link21] values describing their workers. How that
list is constructed is a runtime implementation detail and is not relevant to runtime
consumers.

After moving a value, the runtime calls [`relocate`][__link22], passing where
the value came from and where it now runs.

## Performance, not correctness

Relocation is an optimization, not a guarantee. A value must remain correct if the call
never comes, comes twice, or reports the same source and destination. Missing calls are
normal: a value can reach another thread through `std::thread::spawn`, a channel, or a
runtime that knows nothing about this trait. That may make things slower, but it must
never cause a panic, a deadlock, or a wrong answer.

Call frequency is runtime-dependent. Implementations should make repeated calls with unchanged
relevant coordinates cheap, while avoiding synchronization and other blocking work on every
call.

## What the ids mean

* **Thread id** identifies a live OS thread.
* **[`NumaNode`][__link23]** identifies nearby memory and is shared by threads in the same region.
  It is meaningful across runtimes only when they number regions identically.
* **[`Owner`][__link24]** uniquely identifies the runtime a [`Thread`][__link25] belongs to.

These ids are opaque and need not be consecutive. Use only the coordinate your state
depends on, and store keyed state in a map rather than an indexed array.

## Relation to `Send`

[`ThreadAware`][__link26] requires [`Send`][__link27], and in that order: a value is sent to another thread
first, then told where it landed. [`Send`][__link28] is what makes the move safe, and
[`ThreadAware`][__link29] adds nothing to it.

## Provided implementations

Values with no thread-local state use an empty implementation. Containers forward
relocation to their values, while map keys remain unchanged.

References, sets, `Cow`, and `Arc` have no implementation because relocation would be
ambiguous or could violate their invariants. [`thread_aware`][__link30] provides wrappers for cases
that need an explicit policy, including its per-core [`Arc`][__link31].

## Features

* **`std`** *(default)* - Adds runtime construction support and [`Thread::id`][__link32], which need
  [`ThreadId`][__link33], and implements [`ThreadAware`][__link34] for standard library
  types such as `HashMap`, `Path` and `PathBuf`. Turn it off for `no_std`, which needs only
  `alloc` and pointer-width atomics.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/thread_aware_core">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbk_hQf4SqQD4b5s51ofjYzCAbz8QOpjCYSScbFJ9MdlTDQrBhZIGCcXRocmVhZF9hd2FyZV9jb3JlZTAuMS4w
 [__link0]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link1]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware::relocate
 [__link10]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=NumaNode
 [__link11]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link12]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware::relocate
 [__link13]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link14]: https://docs.rs/thread_aware/latest/thread_aware/derive.ThreadAware.html
 [__link15]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware::relocate
 [__link16]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Thread
 [__link17]: https://doc.rust-lang.org/stable/std/?search=ops::Drop::drop
 [__link18]: https://docs.rs/thread_aware
 [__link19]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Thread
 [__link2]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Thread
 [__link20]: https://docs.rs/thread_aware
 [__link21]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Thread
 [__link22]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware::relocate
 [__link23]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=NumaNode
 [__link24]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Owner
 [__link25]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Thread
 [__link26]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link27]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link28]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link29]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link3]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Thread
 [__link30]: https://docs.rs/thread_aware
 [__link31]: https://docs.rs/thread_aware/latest/thread_aware/struct.Arc.html
 [__link32]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Thread::id
 [__link33]: https://doc.rust-lang.org/stable/std/?search=thread::ThreadId
 [__link34]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link4]: https://doc.rust-lang.org/stable/std/?search=thread::Thread
 [__link5]: https://docs.rs/thread_aware
 [__link6]: https://docs.rs/thread_aware/latest/thread_aware/derive.ThreadAware.html
 [__link7]: https://docs.rs/thread_aware/latest/thread_aware/struct.Arc.html
 [__link8]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Thread
 [__link9]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Owner
