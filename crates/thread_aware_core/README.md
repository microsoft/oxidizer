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
* [`Place`][__link1] records where it now runs: which runtime, which thread, and which memory is
  closest to it.

The crate has no dependencies. It also works without `std`: with default features turned
off, [`Place`][__link2] loses its thread id and keeps [`Owner`][__link3] and [`NumaNode`][__link4]. The companion
`thread_aware` crate provides the conveniences on top: a `#[derive(ThreadAware)]` macro,
wrappers for foreign types, and a per-core `Arc`.

## Why this crate is separate

A crate that names a thread-aware type in its own public API inherits whatever that
type’s crate promises. Keeping the trait and [`Place`][__link5] here, in something small,
dependency-free and slow-moving, lets such crates expose them without taking on the
larger surface. The containers, callbacks, registry and derive support in `thread_aware`
stay free to evolve, and are not meant to appear in a public API. Depend on this crate
directly when only the trait is needed.

## Why relocation exists

Thread-per-core and NUMA-aware runtimes are fast because each worker keeps to itself: it
uses memory close to its own thread, talks to its own I/O driver, and does not
synchronize with other workers. When a value moves to another worker, what used to be
close by is now in the wrong place: a cache line shared between threads, memory in a
distant region, a handle to another thread’s driver.

[`ThreadAware`][__link6] lets that state repair itself. The runtime moves the value, then calls
[`relocate`][__link7] to report where it now lives.

## The two roles

**Library and application authors** implement [`ThreadAware`][__link8], usually through the
`#[derive(ThreadAware)]` macro. They never call [`relocate`][__link9] and
never construct a [`Place`][__link10]; the runtime does both and then invokes the implementation.
It is a callback, like [`Drop::drop`][__link11].

**Runtime authors** construct a [`Place`][__link12] per worker and call
[`relocate`][__link13] after moving a value, passing where it came from and
where it now runs.

A type composed of other types forwards the call to its fields, so one call at the top
reaches everything below it. The derive macro and containers in `thread_aware` do this
automatically.

The example below plays the part of the runtime so the order is visible.

```rust
use std::thread;

use thread_aware_core::{NumaNode, Owner, Place, ThreadAware};

// What a library author writes.
struct Worker {
    thread: Option<thread::ThreadId>,
}

impl ThreadAware for Worker {
    fn relocate(&mut self, _source: Option<&Place>, destination: &Place) {
        self.thread = Some(destination.thread());
    }
}

// What the runtime does.
let here = thread::current().id();
let there = thread::spawn(|| thread::current().id()).join().unwrap();

let owner = Owner::new(1);
let first = Place::new(owner, here, NumaNode::new(0));
let second = Place::new(owner, there, NumaNode::new(1));

let mut worker = Worker { thread: None };

worker.relocate(None, &first); // first placement, no previous place
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

The thread id is `std::thread::ThreadId`. It identifies one thread and nothing else, and
is unique among the threads alive at the same time, so state keyed on it is never shared
by accident, not even between two runtimes in the same process.

[`NumaNode`][__link14] identifies the memory closest to that thread. Unlike the thread id it is
shared: every thread near the same memory reports the same [`NumaNode`][__link15], which is what
makes it suitable for state shared within a region but not across the machine.

That sharing holds only while every runtime in the process numbers the regions
identically, for example from the numbering the operating system reports. Nothing checks
it, and if two runtimes number them differently then state shared between them is wrong,
not merely slow. Share across runtimes only when all of them are under common control.

[`Owner`][__link16] identifies the runtime a place belongs to. Thread ids already distinguish
threads, so this id answers a different question: it lets a value detect that it has
crossed into a different runtime and release anything the previous one owned.

An implementation therefore reads only the ids its state depends on:

* State that must not be shared at all, such as a per-thread cache or a handle to a
  thread-local driver, keys on the thread id and is replaced whenever the thread changes.
* State concerned only with memory locality, such as a buffer pool, keys on [`NumaNode`][__link17]
  and survives a move to another thread near the same memory.
* State owned by the runtime, such as a scheduler handle, also compares [`Owner`][__link18] and is
  released when it changes.

The ids carry no meaning beyond identity. [`Owner`][__link19] and [`NumaNode`][__link20] need not start at
zero or run consecutively, no count is exposed, and the places in use cannot be
enumerated. Per-place state belongs in a map keyed by the id rather than an array indexed
by it.

Without `std` there is no thread id: `Place::new` and `Place::thread` are absent and only
[`Owner`][__link21] and [`NumaNode`][__link22] remain. A `no_std` library can still implement
[`ThreadAware`][__link23] and use whatever it is given; the runtime that drives relocation requires
`std` regardless.

## Relation to `Send`

[`ThreadAware`][__link24] requires [`Send`][__link25], and in that order: a value is sent to another thread
first, then told where it landed. [`Send`][__link26] is what makes the move safe, and
[`ThreadAware`][__link27] adds nothing to it.

## Provided implementations

Types with nothing tied to a place receive an empty implementation: primitives and their
non-zero variants, the place ids, `Duration`, strings, safe function pointers of up to
twelve parameters, and, with the `std` feature, paths.

Containers forward the call to what they hold: [`Option`][__link28], [`Result`][__link29], arrays, slices,
`Vec`, `VecDeque`, `Box`, `Cow`, cells, tuples of up to twelve elements, and map values.
A borrowed `Cow` is taken to owned so that it can be relocated as well.

Map keys are left alone, since altering one could change its hash or ordering and corrupt
the map. Sets are not implemented at all for the same reason, so a `HashSet` or
`BTreeSet` field is not [`ThreadAware`][__link30].

`Arc` is also omitted: whether a shared allocation should stay shared across threads or
be split per thread depends on what it holds. The per-core `Arc` in `thread_aware` covers
the case where splitting is correct.

## Crate features

* The **`std` Cargo feature** *(enabled by default)* provides the thread id half of
  [`Place`][__link31] and implementations for standard library types such as `HashMap`, `Path` and
  `PathBuf`. Turning it off yields a `no_std` build that requires only `alloc`.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/thread_aware_core">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbtnma_MpRRxcbwz5m2G5tKWkbqvbyJnq8I7cbvrOBm74YZX1hZIGCcXRocmVhZF9hd2FyZV9jb3JlZTAuMS4w
 [__link0]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link1]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Place
 [__link10]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Place
 [__link11]: https://doc.rust-lang.org/stable/std/?search=ops::Drop::drop
 [__link12]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Place
 [__link13]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware::relocate
 [__link14]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=NumaNode
 [__link15]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=NumaNode
 [__link16]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Owner
 [__link17]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=NumaNode
 [__link18]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Owner
 [__link19]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Owner
 [__link2]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Place
 [__link20]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=NumaNode
 [__link21]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Owner
 [__link22]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=NumaNode
 [__link23]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link24]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link25]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link26]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link27]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link28]: https://doc.rust-lang.org/stable/std/option/enum.Option.html
 [__link29]: https://doc.rust-lang.org/stable/std/result/struct.Result.html
 [__link3]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Owner
 [__link30]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link31]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Place
 [__link4]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=NumaNode
 [__link5]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Place
 [__link6]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link7]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware::relocate
 [__link8]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link9]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware::relocate
