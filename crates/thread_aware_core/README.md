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

Stable foundations for moving thread-isolated state between execution contexts.

This crate contains the small API shared by thread-aware libraries:

* [`ThreadAware`][__link0] notifies a value that it has moved to a different location.
* [`Location`][__link1] identifies the execution context — topology, core and memory
  region — that a value has moved to.

The crate has no dependencies and is always `no_std`. Its opt-in `std`
feature adds implementations for standard-library types such as `HashMap`
and `&Path`.

Ergonomics built on this foundation — a `#[derive(ThreadAware)]` macro, wrappers for
foreign types, and a per-core `Arc` — live in the companion `thread_aware` crate.
Depend on `thread_aware_core` directly when you only need to *implement* the trait,
so that your own public API does not pull in the larger crate.

## Why relocation exists

Thread-per-core and NUMA-aware runtimes get their performance from locality: a worker
touches memory in its own region, talks to its own I/O driver, and avoids synchronizing
with its peers. When a value moves from one worker to another, state that used to be
local becomes remote — a cache line now shared across cores, an allocation now on a
foreign NUMA node, a handle now pointing at another thread’s driver.

[`ThreadAware`][__link2] is the notification that lets such state repair itself. The runtime
moves the value, then calls [`relocate`][__link3] to say *“you now live
here”*, and the value re-establishes whatever locality it cares about.

## Theory of operation

Two distinct roles share this trait, and it matters which one you are in.

**Library and application authors — the common case.** You *implement* [`ThreadAware`][__link4]
on your types, usually through the `#[derive(ThreadAware)]` macro in the `thread_aware`
crate. You do **not** call [`relocate`][__link5] yourself and you do not
construct a [`Location`][__link6]. A thread-aware runtime does that for you automatically
whenever it moves your value between workers; your implementation simply gets invoked.
Treat [`relocate`][__link7] as a callback, in the same way you never call
[`Drop::drop`][__link8] by hand.

**Runtime authors — the rare case.** A runtime establishes the topology, constructs the
[`Location`][__link9] values that describe its workers, and drives relocation by calling
[`relocate`][__link10] after it has moved a value, passing the location the
value came from (when known) and the one it now runs on. Only code that owns the
placement of work needs to do this.

Composite types forward the notification to their parts, so one call at the root of an
object graph reaches every field that cares. That is what the `thread_aware` derive
macro generates, and what the collection and container implementations in this crate do.

The example below plays the part of the runtime explicitly so that the sequence is
visible. In real code the two `relocate` calls come from the runtime, not from you.

```rust
use thread_aware_core::{Core, Location, MemoryRegion, ThreadAware, Topology};

// What a library author writes: an implementation, and nothing else.
struct Worker {
    core: Option<Core>,
}

impl ThreadAware for Worker {
    fn relocate(&mut self, _source: Option<&Location>, destination: &Location) {
        self.core = Some(destination.core());
    }
}

// What a runtime does on the library author's behalf.
let topology = Topology::from(1);
let first = Location::new(topology, Core::from(0), MemoryRegion::from(0));
let second = Location::new(topology, Core::from(3), MemoryRegion::from(1));

let mut worker = Worker { core: None };

// Initial placement; the previous location is unknown.
worker.relocate(None, &first);

// Later, the runtime migrates the worker to another core.
worker.relocate(Some(&first), &second);

assert_eq!(worker.core, Some(Core::from(3)));
```

## Performance, not correctness

Relocation is a cooperative performance optimization rather than a correctness boundary.
Implementations must remain correct if a relocation notification is omitted, repeated,
or reports the same source and destination.

Missed notifications are expected in practice: a value can reach another thread through
`std::thread::spawn`, a channel, or any runtime that does not participate in this
protocol. The only permitted consequence is degraded locality — never a panic, a
deadlock, or a wrong answer.

Relocation is also not a hot path. The expected pattern is one relocation per object
graph per job or request — an incoming request is routed to a worker and its dependency
graph is relocated onto that worker — after which the steady-state hot path simply uses
the now-local state. Implementations should therefore favor avoiding synchronization
over shaving cycles.

## Coordinate space

[`Core`][__link11] and [`MemoryRegion`][__link12] name hardware on the physical machine rather than indexing
the worker list of one runtime. Their values are intended to be meaningful process-wide:
two runtimes on the same machine that both use core 2 report the same [`Core`][__link13], and state
keyed by it can legitimately be shared between them. Preserving that sharing is why the
API exposes identities rather than re-numbered indices.

This crate cannot enforce it. [`Core::from`][__link14] and [`MemoryRegion::from`][__link15] accept any `u16`,
so cross-runtime sharing is sound only while every runtime in the process derives these
values from the same physical numbering — the operating system’s logical processor and
NUMA node ids, say. If two runtimes number the same hardware differently, state shared
between them on a [`Core`][__link16] key is wrong, not merely slow. Share across runtimes only when
you control every runtime in the process; otherwise treat the state as runtime-bound.

[`Topology`][__link17] identifies the runtime that produced the location. It does not scope the
hardware coordinates; it tells an implementation whether it is still inside the runtime
whose resources it holds.

Which coordinates an implementation reads is its own choice:

* Hardware-keyed state — a per-core cache, a region-local buffer pool — can key on
  [`Core`][__link18] or [`MemoryRegion`][__link19] alone and stay valid across topologies, provided it is
  backed by resources that outlive any single runtime.
* Runtime-bound state — a task scheduler, a handle to a thread-local I/O driver, memory
  allocated by a particular runtime — must also compare [`Topology`][__link20] and detach when
  it changes.

State keyed on hardware but *backed* by runtime-owned resources counts as runtime-bound;
when in doubt classify it that way, since the only cost is a re-acquire that a purely
hardware-keyed value could have skipped.

The values themselves are opaque identities:

* They are not promised to start at zero, to be contiguous, or to be bounded by the
  number of cores or regions in use. A machine may report cores `1` and `399` and
  nothing in between.
* No count is exposed, so the set of live locations is not enumerable through this API.
  Implementations that need per-location storage should key a map by the id rather than
  index into a pre-sized array. That is affordable because relocation is not a hot path:
  the lookup happens when a value moves, not on every use of the value.

## Relation to `Send`

[`ThreadAware`][__link21] requires [`Send`][__link22] as a supertrait, and the two happen in that order: a
value is first sent to another thread, and only then told where it landed. [`Send`][__link23]
remains the safety property; [`ThreadAware`][__link24] adds nothing to it.

## Thread versus core semantics

This crate targets thread-per-core runtimes, where each worker thread is pinned to one
logical processor, so “moved to another thread” and “moved to another core” describe the
same event. [`Location`][__link25] cannot express more than one worker per core: a runtime that
runs several threads per core, or leaves threads unpinned, has to give each worker a
distinct [`Core`][__link26], which forfeits sharing between workers that really do sit on the same
processor. [`Location`][__link27] has no way to say “not pinned to this dimension”.

## Provided implementations

[`ThreadAware`][__link28] is implemented for types with no location-dependent state (primitives,
location identifiers, `Duration`, strings, safe function pointers, and, with the `std`
feature, paths). It is forwarded through container types such as [`Option`][__link29], [`Result`][__link30],
arrays, slices, `Vec`, `VecDeque`, `Box`, `Cow`, cells, tuples up to twelve elements and
map values. A `Cow` forwards only when it is `Cow::Owned`; a borrowed one is left alone.
Map keys are deliberately not relocated, because changing their equality,
hashing or ordering would violate the collection’s invariants. For the same reason no
set implementation is provided at all: a set has only elements, so a `HashSet` or
`BTreeSet` field is simply not [`ThreadAware`][__link31]. Hold location-sensitive set contents in a
map, or in a newtype you relocate explicitly.

`Arc` is deliberately **not** implemented. Whether a shared allocation should stay shared
across cores or be split per core is a policy decision that depends on what the `Arc`
holds — read-mostly data is often fine to share — so no blanket implementation can make
it. The choice belongs to the type holding the `Arc`; the `thread_aware` crate provides a
per-core `Arc` for when splitting is the right answer.

## Crate features

* The **`std` Cargo feature** *(off by default)* adds implementations for
  standard-library types such as `HashMap`, `Path` and `PathBuf`. Without it the crate
  needs only `alloc`.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/thread_aware_core">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbPpf1myAraf0bGcRZ6NeIVXsbctzYtSggUawbtEGAPL6tDjVhZIGCcXRocmVhZF9hd2FyZV9jb3JlZTAuMS4w
 [__link0]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/trait.ThreadAware.html
 [__link1]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Location
 [__link10]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware::relocate
 [__link11]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Core
 [__link12]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=MemoryRegion
 [__link13]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Core
 [__link14]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Core::from
 [__link15]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=MemoryRegion::from
 [__link16]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Core
 [__link17]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Topology
 [__link18]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Core
 [__link19]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=MemoryRegion
 [__link2]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/trait.ThreadAware.html
 [__link20]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Topology
 [__link21]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/trait.ThreadAware.html
 [__link22]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link23]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link24]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/trait.ThreadAware.html
 [__link25]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Location
 [__link26]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Core
 [__link27]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Location
 [__link28]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/trait.ThreadAware.html
 [__link29]: https://doc.rust-lang.org/stable/std/option/enum.Option.html
 [__link3]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware::relocate
 [__link30]: https://doc.rust-lang.org/stable/std/result/struct.Result.html
 [__link31]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/trait.ThreadAware.html
 [__link4]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/trait.ThreadAware.html
 [__link5]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware::relocate
 [__link6]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Location
 [__link7]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=ThreadAware::relocate
 [__link8]: https://doc.rust-lang.org/stable/std/?search=ops::Drop::drop
 [__link9]: https://docs.rs/thread_aware_core/0.1.0/thread_aware_core/?search=Location
