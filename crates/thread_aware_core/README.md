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

Lets values adapt when a runtime moves them to another CPU core.

This crate contains the small API shared by thread-aware libraries:

* [`ThreadAware`][__link0] tells a value that it has moved.
* [`Location`][__link1] says where it now runs: which runtime, which core, and which region of
  memory sits closest to that core.

The crate has no dependencies and is always `no_std`. The companion `thread_aware` crate
adds the conveniences: a `#[derive(ThreadAware)]` macro, wrappers for foreign types, and
a per-core `Arc`. Depend on this crate directly if you only need to implement the trait.

## Why relocation exists

Thread-per-core and NUMA-aware runtimes are fast because each worker keeps to itself: it
uses memory close to its own core, talks to its own I/O driver, and does not synchronize
with other workers. When a value moves to another worker, what used to be close by is now
in the wrong place: a cache line shared between cores, memory in a distant region, a
handle to another thread’s driver.

[`ThreadAware`][__link2] lets that state fix itself. The runtime moves the value, then calls
[`relocate`][__link3] to say where it now lives.

## The two roles

**If you write a library or an application**, you implement [`ThreadAware`][__link4], usually with
the `#[derive(ThreadAware)]` macro. You never call [`relocate`][__link5]
and never build a [`Location`][__link6]; the runtime does both, and calls your implementation
afterwards. It is a callback, like [`Drop::drop`][__link7].

**If you write a runtime**, you build a [`Location`][__link8] per worker and call
[`relocate`][__link9] after moving a value, passing where it came from and
where it now runs.

A type made of other types passes the call on to its fields, so one call at the top
reaches everything below it. The derive macro and the containers here do that for you.

The example below plays the part of the runtime so the order is visible.

```rust
use thread_aware_core::{Core, Location, MemoryRegion, ThreadAware, Topology};

// What a library author writes.
struct Worker {
    core: Option<Core>,
}

impl ThreadAware for Worker {
    fn relocate(&mut self, _source: Option<&Location>, destination: &Location) {
        self.core = Some(destination.core());
    }
}

// What the runtime does.
let topology = Topology::from(1);
let first = Location::new(topology, Core::from(0), MemoryRegion::from(0));
let second = Location::new(topology, Core::from(3), MemoryRegion::from(1));

let mut worker = Worker { core: None };

worker.relocate(None, &first); // first placement, no previous location
worker.relocate(Some(&first), &second); // moved to another core

assert_eq!(worker.core, Some(Core::from(3)));
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

[`Core`][__link10] and [`MemoryRegion`][__link11] name hardware, not slots in a worker list, so two runtimes
on the same machine both report core 2 as the same [`Core`][__link12] and can share state keyed on
it. That only holds while they derive the ids the same way, for example from the
numbering the operating system reports. Nothing checks it, because [`Core::from`][__link13] accepts
any `u16`, and if two runtimes number hardware differently then state shared between them
is wrong, not just slow. Share across runtimes only when you control every one of them.

[`Topology`][__link14] says which runtime produced the location. It does not change what the other
two mean; it tells you whether you are still inside the runtime that gave you your
resources. So use only the ids your state depends on:

* State tied to hardware, such as a per-core cache, can use [`Core`][__link15] or [`MemoryRegion`][__link16]
  alone, and survives a move between runtimes as long as what backs it is not owned by
  one of them.
* State tied to a runtime, such as a scheduler, a driver handle, or memory it allocated,
  has to check [`Topology`][__link17] too and let go when it changes. When in doubt, assume this.

The ids mean nothing beyond identity. They need not start at zero or run consecutively,
there is no count, and you cannot list them. Keep per-location state in a map keyed by
the id rather than an array you index into.

## Relation to `Send`

[`ThreadAware`][__link18] requires [`Send`][__link19], and in that order: a value is sent to another thread
first, then told where it landed. [`Send`][__link20] is what makes the move safe, and
[`ThreadAware`][__link21] adds nothing to it.

## Threads and cores

This crate assumes one worker per core, so “moved to another thread” and “moved to
another core” mean the same thing. A [`Location`][__link22] cannot describe two workers on one
core: a runtime that puts several threads on a core, or leaves them unpinned, has to give
each worker its own [`Core`][__link23] id, and those workers then no longer share per-core state.
There is no way to say “not pinned”.

## Provided implementations

Types with nothing tied to a location get an empty implementation: primitives, the
location ids, `Duration`, strings, safe function pointers, and, with the `std` feature,
paths.

Containers pass the call through to what they hold: [`Option`][__link24], [`Result`][__link25], arrays,
slices, `Vec`, `VecDeque`, `Box`, `Cow`, cells, tuples of up to twelve elements, and map
values. A `Cow` only forwards when it owns its data.

Map keys are left alone, since changing one could change its hash or ordering and break
the map. Sets are not implemented at all for the same reason, so a `HashSet` or
`BTreeSet` field is simply not [`ThreadAware`][__link26].

`Arc` is left out too: whether a shared allocation should stay shared across cores or be
split per core depends on what is inside it. Use the per-core `Arc` in `thread_aware`
when splitting is the right answer.

## Crate features

* The **`std` Cargo feature** *(off by default)* adds implementations for standard
  library types such as `HashMap`, `Path` and `PathBuf`. Without it the crate needs only
  `alloc`.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/thread_aware_core">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbw6ILdJBLgS0bFKDldqXnrA8bAU-5-5cMOtcbMS2AklbWB3thZIGCcXRocmVhZF9hd2FyZV9jb3JlZTAuMS4w
 [__link0]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/trait.ThreadAware.html
 [__link1]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Location
 [__link10]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Core
 [__link11]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=MemoryRegion
 [__link12]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Core
 [__link13]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Core::from
 [__link14]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Topology
 [__link15]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Core
 [__link16]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=MemoryRegion
 [__link17]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Topology
 [__link18]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/trait.ThreadAware.html
 [__link19]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link2]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/trait.ThreadAware.html
 [__link20]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link21]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/trait.ThreadAware.html
 [__link22]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Location
 [__link23]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Core
 [__link24]: https://doc.rust-lang.org/stable/std/option/enum.Option.html
 [__link25]: https://doc.rust-lang.org/stable/std/result/struct.Result.html
 [__link26]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/trait.ThreadAware.html
 [__link3]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware::relocate
 [__link4]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/trait.ThreadAware.html
 [__link5]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware::relocate
 [__link6]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Location
 [__link7]: https://doc.rust-lang.org/stable/std/?search=ops::Drop::drop
 [__link8]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Location
 [__link9]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware::relocate
