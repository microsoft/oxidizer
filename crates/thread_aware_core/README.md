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

* [`ThreadAware`][__link0] notifies a value that it has moved.
* [`Thread`][__link1] records where it now runs: which runtime, which OS thread, and which memory is
  closest to it.

[`Thread`][__link2] is a coordinate, not a handle: a runtime builds one to describe where a value
is running, and it owns no operating-system resource. It is unrelated to
[`std::thread::Thread`][__link3], which is a handle to a live OS thread. Naming both in one module
requires aliasing one of them.

## The `thread_aware` family

* **`thread_aware_core`** (this crate) — the vocabulary that two unrelated libraries must
  agree on before either can relocate a value defined by the other. Deliberately small and
  slow-moving, so naming [`ThreadAware`][__link4] or [`Thread`][__link5] in your own public API costs you
  nothing later.
* **[`thread_aware`][__link6]** — the utilities that make relocation convenient: a
  [`#[derive(ThreadAware)]`][__link7] macro, wrappers for foreign types, a per-core
  [`Arc`][__link8], containers and registries. Free to evolve, and not meant to appear in a
  public API.

Depend on this crate directly when all you need is the trait. It adds nothing to your
dependency graph, and works without `std`: with default features turned off, [`Thread`][__link9]
loses its thread id component and keeps [`Owner`][__link10] and [`NumaNode`][__link11].

## Why relocation exists

Thread-per-core and NUMA-aware runtimes are fast because each worker keeps to itself: it
uses memory close to its own thread, talks to its own I/O driver, and does not
synchronize with other workers. When a value moves to another worker, what used to be
close by is now in the wrong place: a cache line shared between threads, memory in a
distant region, a handle to another thread’s driver.

[`ThreadAware`][__link12] lets that state repair itself. The runtime moves the value, then calls
[`relocate`][__link13] to report where it now lives. Relocation has two
sides, and most code sits on only one of them.

## Library authors: implementing the trait

**Library and application authors** implement [`ThreadAware`][__link14], usually through the
[`#[derive(ThreadAware)]`][__link15] macro. They never call
[`relocate`][__link16] and never construct a [`Thread`][__link17]; the runtime does
both and then invokes the implementation. It is a callback, like [`Drop::drop`][__link18].

The derive lives in [`thread_aware`][__link19], so a library that wants it depends on that crate.
Only the trait and [`Thread`][__link20] cross the public boundary, and both come from here, so the
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
reaches everything below it. Callers of `Encoder` never name [`thread_aware`][__link21].

## Runtime authors: driving relocation

**Runtime authors** construct a [`Thread`][__link22] per worker and call
[`relocate`][__link23] after moving a value, passing where it came from and
where it now runs. The example below plays the part of the runtime so the order is
visible.

```rust
use std::thread;

use thread_aware_core::{NumaNode, Owner, Thread, ThreadAware};

// What a library author writes.
struct Worker {
    thread: Option<thread::ThreadId>,
}

impl ThreadAware for Worker {
    fn relocate(&mut self, _source: Option<&Thread>, destination: &Thread) {
        self.thread = Some(destination.id());
    }
}

// What the runtime does.
let here = thread::current().id();
let there = thread::spawn(|| thread::current().id()).join().unwrap();

let owner = Owner::new(2);
let first = Thread::new(owner, here, NumaNode::new(0));
let second = Thread::new(owner, there, NumaNode::new(1));

let mut worker = Worker { thread: None };

worker.relocate(None, &first); // first placement, no previous `Thread`
worker.relocate(Some(&first), &second); // moved to another thread

assert_eq!(worker.thread, Some(there));
```

## Performance, not correctness

Relocation is an optimization, not a guarantee. A value must remain correct if the call
never comes, comes twice, or reports the same source and destination. Missing calls are
normal: a value can reach another thread through `std::thread::spawn`, a channel, or a
runtime that knows nothing about this trait. That may make things slower, but it must
never cause a panic, a deadlock, or a wrong answer.

Nor is it a hot path. Expect roughly one relocation per object graph per job or request,
after which the value is used normally. Avoiding synchronization matters more than saving a
few cycles.

## What the ids mean

* **Thread id** — a `std::thread::ThreadId`, unique among the threads alive at once, so
  state keyed on it is never shared by accident, not even between two runtimes in one
  process.
* **[`NumaNode`][__link24]** — the memory closest to that thread. Unlike the thread id it is
  *shared*: every thread near the same memory reports the same node, which is what suits
  it to state shared within a region but not across the machine. That holds only while
  every runtime numbers the regions identically. Nothing checks it, and runtimes that
  disagree make shared state wrong rather than merely slow.
* **[`Owner`][__link25]** — the runtime a [`Thread`][__link26] belongs to. Every new owner is unique, so two
  live runtimes never share one. It lets a value detect that it has crossed into a
  different runtime and release anything the previous one owned.

An implementation reads only the ids its state depends on. A per-thread cache or a handle
to a thread-local driver keys on the thread id; a buffer pool keys on [`NumaNode`][__link27] and
survives a move to another thread near the same memory; anything the runtime owns compares
[`Owner`][__link28].

The ids carry no meaning beyond identity: they need not start at zero or run
consecutively, and the [`Thread`][__link29]s in use cannot be enumerated. State keyed on any of them
belongs in a map rather than an array indexed by it. [`Owner::min_threads`][__link30] is the one
number on offer, and it is a floor to pre-size against, not a bound to index against.

Without `std` there is no [`ThreadId`][__link31]: `Thread::new` and
`Thread::id` are absent and only [`Owner`][__link32] and [`NumaNode`][__link33] remain. A `no_std` library can
still implement [`ThreadAware`][__link34] and use whatever it is given; the runtime that drives
relocation requires `std` regardless.

## Relation to `Send`

[`ThreadAware`][__link35] requires [`Send`][__link36], and in that order: a value is sent to another thread
first, then told where it landed. [`Send`][__link37] is what makes the move safe, and
[`ThreadAware`][__link38] adds nothing to it.

## Provided implementations

Types with nothing tied to a thread receive an empty implementation: primitives and their
non-zero variants, the thread ids, `Duration`, strings, safe function pointers of up to
twelve parameters, and, with the `std` feature, paths.

Containers forward the call to what they hold: [`Option`][__link39], [`Result`][__link40], arrays, slices,
`Vec`, `VecDeque`, `Box`, cells, tuples of up to twelve elements, and map values.

References are not [`ThreadAware`][__link41]. Relocating through one would adapt something the value
only borrows, and whoever owns it is relocated on its own account.

Map keys are left alone, since altering one could change its hash or ordering and corrupt
the map. Sets are not implemented at all for the same reason, so a `HashSet` or
`BTreeSet` field is not [`ThreadAware`][__link42].

`Cow` is omitted for now: relocating a borrowed one has to clone it into owned storage
first, which is a surprising amount of work to hide behind a hint.

`Arc` is also omitted: whether a shared allocation should stay shared across threads or
be split per thread depends on what it holds. The per-core [`Arc`][__link43] in
[`thread_aware`][__link44] covers the case where splitting is correct.

## Features

* **`std`** *(default)* - Adds [`Thread::new`][__link45] and [`Thread::id`][__link46], which need
  [`ThreadId`][__link47], and implements [`ThreadAware`][__link48] for standard library
  types such as `HashMap`, `Path` and `PathBuf`. Turn it off for `no_std`, which needs
  only `alloc` and pointer-width atomics.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/thread_aware_core">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbwWQI47ZIOnYb_X5SIRoaYBkb3kaQqZrTGssbXSoz6MMTTcdhZIGCcXRocmVhZF9hd2FyZV9jb3JlZTAuMS4w
 [__link0]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link1]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Thread
 [__link10]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Owner
 [__link11]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=NumaNode
 [__link12]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link13]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware::relocate
 [__link14]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link15]: https://docs.rs/thread_aware/latest/thread_aware/derive.ThreadAware.html
 [__link16]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware::relocate
 [__link17]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Thread
 [__link18]: https://doc.rust-lang.org/stable/std/?search=ops::Drop::drop
 [__link19]: https://docs.rs/thread_aware
 [__link2]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Thread
 [__link20]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Thread
 [__link21]: https://docs.rs/thread_aware
 [__link22]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Thread
 [__link23]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware::relocate
 [__link24]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=NumaNode
 [__link25]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Owner
 [__link26]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Thread
 [__link27]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=NumaNode
 [__link28]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Owner
 [__link29]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Thread
 [__link3]: https://doc.rust-lang.org/stable/std/?search=thread::Thread
 [__link30]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Owner::min_threads
 [__link31]: https://doc.rust-lang.org/stable/std/?search=thread::ThreadId
 [__link32]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Owner
 [__link33]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=NumaNode
 [__link34]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link35]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link36]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link37]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link38]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link39]: https://doc.rust-lang.org/stable/std/option/enum.Option.html
 [__link4]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link40]: https://doc.rust-lang.org/stable/std/result/struct.Result.html
 [__link41]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link42]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link43]: https://docs.rs/thread_aware/latest/thread_aware/struct.Arc.html
 [__link44]: https://docs.rs/thread_aware
 [__link45]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Thread::new
 [__link46]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Thread::id
 [__link47]: https://doc.rust-lang.org/stable/std/?search=thread::ThreadId
 [__link48]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link5]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Thread
 [__link6]: https://docs.rs/thread_aware
 [__link7]: https://docs.rs/thread_aware/latest/thread_aware/derive.ThreadAware.html
 [__link8]: https://docs.rs/thread_aware/latest/thread_aware/struct.Arc.html
 [__link9]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Thread
