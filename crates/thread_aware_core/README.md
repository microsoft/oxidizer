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

Lets values adapt when a runtime moves them to another thread.

This crate contains the small API shared by thread-aware libraries:

* [`ThreadAware`][__link0] tells a value that it has moved.
* [`Place`][__link1] says where it now runs: which runtime, which thread, and which memory is
  closest to it.

The crate has no dependencies. It also works without `std`: turn off default features,
and [`Place`][__link2] loses its thread id, keeping [`Origin`][__link3] and [`NumaNode`][__link4]. The companion
`thread_aware` crate adds the conveniences: a `#[derive(ThreadAware)]` macro, wrappers
for foreign types, and a per-core `Arc`. Depend on this crate directly if you only need
to implement the trait.

## Why relocation exists

Thread-per-core and NUMA-aware runtimes are fast because each worker keeps to itself: it
uses memory close to its own thread, talks to its own I/O driver, and does not
synchronize with other workers. When a value moves to another worker, what used to be
close by is now in the wrong place: a cache line shared between threads, memory in a
distant region, a handle to another thread’s driver.

[`ThreadAware`][__link5] lets that state fix itself. The runtime moves the value, then calls
[`relocate`][__link6] to say where it now lives.

## The two roles

**If you write a library or an application**, you implement [`ThreadAware`][__link7], usually with
the `#[derive(ThreadAware)]` macro. You never call [`relocate`][__link8]
and never build a [`Place`][__link9]; the runtime does both, and calls your implementation
afterwards. It is a callback, like [`Drop::drop`][__link10].

**If you write a runtime**, you build a [`Place`][__link11] per worker and call
[`relocate`][__link12] after moving a value, passing where it came from and
where it now runs.

A type made of other types passes the call on to its fields, so one call at the top
reaches everything below it. The derive macro and the containers here do that for you.

The example below plays the part of the runtime so the order is visible.

```rust
use std::thread;

use thread_aware_core::{NumaNode, Origin, Place, ThreadAware};

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

let origin = Origin::from(1);
let first = Place::new(origin, here, NumaNode::from(0));
let second = Place::new(origin, there, NumaNode::from(1));

let mut worker = Worker { thread: None };

worker.relocate(None, &first); // first placement, no previous place
worker.relocate(Some(&first), &second); // moved to another thread

assert_eq!(worker.thread, Some(there));
```

## Performance, not correctness

Relocation is an optimization, not a guarantee. Your value has to stay correct if the
call never comes, comes twice, or reports the same source and destination. Missing calls
are normal: a value can reach another thread through `std::thread::spawn`, a channel, or
a runtime that knows nothing about this trait. That may make things slower, but it must
never cause a panic, a deadlock, or a wrong answer.

It is not a hot path either. Expect about one relocation per object graph per job or
request, after which the value is simply used. Prefer avoiding synchronization over
saving a few cycles.

## What the ids mean

The thread id is `std::thread::ThreadId`. It identifies one thread and nothing else, and
is unique among the threads alive at the same time, so state keyed on it is never shared
by accident, not even between two runtimes in the same process.

[`NumaNode`][__link13] identifies the memory closest to that thread. Unlike the thread id it is
shared: every thread near the same memory reports the same [`NumaNode`][__link14]. That is what
makes it useful for state you want to share within a region but not across the machine.

That sharing only works while every runtime in the process numbers the regions the same
way, for example from the numbering the operating system reports. Nothing checks it, and
if two runtimes number them differently then state shared between them is wrong, not just
slow. Share across runtimes only when you control every one of them.

[`Origin`][__link15] identifies the runtime that produced the place. Thread ids already tell
threads apart, so this is about ownership: it lets a value notice that it has crossed
into a different runtime and release anything the old one owned.

So use only the ids your state depends on:

* State that must not be shared at all, such as a per-thread cache or a handle to a
  thread-local driver, keys on the thread id and is replaced whenever the thread changes.
* State that only cares about memory locality, such as a buffer pool, keys on
  [`NumaNode`][__link16] and survives a move to another thread near the same memory.
* State owned by the runtime, such as a scheduler handle, also checks [`Origin`][__link17] and lets
  go when it changes.

The ids mean nothing beyond identity. [`Origin`][__link18] and [`NumaNode`][__link19] need not start at zero
or run consecutively, there is no count, and you cannot list the places in use. Keep
per-place state in a map keyed by the id rather than an array you index into.

Without `std` there is no thread id: `Place::new` and `Place::thread` are gone and only
[`Origin`][__link20] and [`NumaNode`][__link21] remain. A `no_std` library can still implement [`ThreadAware`][__link22]
and use whatever it is handed; the runtime that drives relocation needs `std` anyway.

## Relation to `Send`

[`ThreadAware`][__link23] requires [`Send`][__link24], and in that order: a value is sent to another thread
first, then told where it landed. [`Send`][__link25] is what makes the move safe, and
[`ThreadAware`][__link26] adds nothing to it.

## Provided implementations

Types with nothing tied to a place get an empty implementation: primitives and their
non-zero variants, the place ids, `Duration`, strings, safe function pointers of up to
twelve parameters, and, with the `std` feature, paths.

Containers pass the call through to what they hold: [`Option`][__link27], [`Result`][__link28], arrays,
slices, `Vec`, `VecDeque`, `Box`, `Cow`, cells, tuples of up to twelve elements, and map
values. A `Cow` only forwards when it owns its data.

Map keys are left alone, since changing one could change its hash or ordering and break
the map. Sets are not implemented at all for the same reason, so a `HashSet` or
`BTreeSet` field is simply not [`ThreadAware`][__link29].

`Arc` is left out too: whether a shared allocation should stay shared across threads or
be split per thread depends on what is inside it. Use the per-core `Arc` in
`thread_aware` when splitting is the right answer.

## Crate features

* The **`std` Cargo feature** *(enabled by default)* provides the thread id half of
  [`Place`][__link30] and implementations for standard library types such as `HashMap`, `Path` and
  `PathBuf`. Turn it off for `no_std`; the crate then needs only `alloc`.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/thread_aware_core">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbfCv1T73cY1MbZtIIDN51f48bRkNe3vWDEewbZruvF3exEWBhZIGCcXRocmVhZF9hd2FyZV9jb3JlZTAuMS4w
 [__link0]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link1]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Place
 [__link10]: https://doc.rust-lang.org/stable/std/?search=ops::Drop::drop
 [__link11]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Place
 [__link12]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware::relocate
 [__link13]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=NumaNode
 [__link14]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=NumaNode
 [__link15]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Origin
 [__link16]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=NumaNode
 [__link17]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Origin
 [__link18]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Origin
 [__link19]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=NumaNode
 [__link2]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Place
 [__link20]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Origin
 [__link21]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=NumaNode
 [__link22]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link23]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link24]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link25]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link26]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link27]: https://doc.rust-lang.org/stable/std/option/enum.Option.html
 [__link28]: https://doc.rust-lang.org/stable/std/result/struct.Result.html
 [__link29]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link3]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Origin
 [__link30]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Place
 [__link4]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=NumaNode
 [__link5]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link6]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware::relocate
 [__link7]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware
 [__link8]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware::relocate
 [__link9]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Place
