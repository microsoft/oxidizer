<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Rallocator Logo" width="96">

# Rallocator

[![crate.io](https://img.shields.io/crates/v/rallocator.svg)](https://crates.io/crates/rallocator)
[![docs.rs](https://docs.rs/rallocator/badge.svg)](https://docs.rs/rallocator)
[![MSRV](https://img.shields.io/crates/msrv/rallocator)](https://crates.io/crates/rallocator)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

A pure-Rust, high-performance allocator integrated with [`allocation_hints`][__link0].

## Supported platforms

`rallocator` currently supports Windows and Linux. Other operating systems
are outside the crate’s public support contract and intentionally fail to
compile. Miri uses an internal test backend and is not a production support
target.

## Usage

### General use

Install the standard configuration as the process-global allocator:

```rust
rallocator::rallocator!();
rallocator::initialize();
```

The macro declares the required `#[global_allocator]` static and contains
the unsafe call to [`Rallocator::new`][__link1], whose process-wide
same-configuration invariant it establishes by construction. [`initialize`][__link2]
registers rallocator with [`allocation_hints`][__link3] before heap or domain APIs are
used.

Ordinary allocations then use each thread’s implicit general heap. To group
related allocations, create a [`Heap`][__link4] and
activate it for a synchronous scope:

```rust
use allocation_hints::heap::{Heap, bump};
use allocation_hints::with_hint;

rallocator::rallocator!();
rallocator::initialize();

let heap = Heap::bump(bump::Options::new());
let values = with_hint(&heap, || vec![1, 2, 3]);
assert_eq!(values.len(), 3);
```

### Telemetry

Telemetry is opt-in at compile time. Define a [`config::Config`][__link5] with
aggregate or caller tracking enabled, then pass it to [`rallocator!`][__link6]:

```rust
use rallocator::telemetry::{snapshot, stats, track_callers};

rallocator::config!(Telemetry {
    track_aggregates: true,
    track_callers: true,
});
rallocator::rallocator!(Telemetry);

fn main() -> std::io::Result<()> {
    rallocator::initialize();
    track_callers(true);
    let mut values = vec![1, 2, 3];
    values[0] += 1;
    std::hint::black_box(&values);
    drop(values);
    track_callers(false);

    snapshot()
        .expect("telemetry tracking is enabled")
        .write_file("snapshot.rallocator")?;

    Ok(())
}
```

Convert the snapshot to HTML with `rallocator_cli snapshot html snapshot.rallocator`.

Aggregate counters remain active for the process lifetime. Caller tracking
can be enabled only around the interval of interest to limit its overhead;
snapshots include allocator topology, counters, and retained caller events.
The default `caller-symbolization` feature resolves captured instruction
pointers through the optional `backtrace` dependency. Disabling default
features retains caller tracking and raw addresses without in-process symbol
resolution.

## Design guide

Use the implicit thread-local general heap for ordinary allocations.
Introduce explicit general heaps when you want locality boundaries without
changing the mixed-size allocation model. Use bump heaps for phase-bounded
work where individual frees are rare and bulk reclamation matters more than
per-allocation reuse. Use separate [`Domain`][__link7]
values only when heaps must stop sharing allocator-managed regions.

## Implementation guide

1. Choose a [`config::Config`][__link8] and, if needed, custom
   [`tunables::Tunables`][__link9].
1. Install [`rallocator!`][__link10] exactly once as the process-global allocator.
1. Call [`initialize`][__link11] near the start of `main` before creating
   allocation-hint heaps or domains.
1. Route special-purpose allocations with
   [`allocation_hints::with_hint`][__link12] and [`allocation_hints::heap::Heap`][__link13].
1. Enable telemetry only when you need it: aggregate counters are a
   compile-time choice, while caller tracking is also runtime-gated.

Invalid tunables fail early when [`Rallocator::new`][__link14] is instantiated: the
size-class layout must be well-formed and the partial-slab scan limit must
be non-zero.

## Internals

Rallocator is organized as a hierarchy:

* A [`Domain`][__link15] owns one or more 1 GiB virtual-memory
  **regions**, divided into 64 KiB **slices**.
* A domain can serve multiple [`Heap`][__link16] instances.
  Each heap keeps its own allocation state while drawing backing memory from
  its domain.
* General heaps use slices for **locality segments** containing 32 KiB
  **slabs**, and for **medium spans** covering one or more slices. Bump heaps
  use slices as **bump chunks**.
* Slabs contain same-sized **blocks**, which are the allocation slots returned
  for small requests. Large or highly aligned requests bypass this hierarchy
  and use direct operating-system mappings.

<div style="overflow-x: auto">
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1100 680"
     role="img" aria-labelledby="allocator-layout-title"
     style="width: 100%; min-width: 820px; max-width: 1100px">
  <title id="allocator-layout-title">Allocator memory layout</title>
  <g fill="none" stroke="currentColor" stroke-width="2">
    <rect x="15" y="15" width="1070" height="650" rx="10"/>
    <rect x="40" y="60" width="775" height="410" rx="8"/>
    <rect x="840" y="60" width="220" height="160" rx="8"/>
    <rect x="840" y="245" width="220" height="160" rx="8"/>
    <rect x="65" y="110" width="725" height="75" rx="5"/>
    <line x1="246" y1="110" x2="246" y2="185"/>
    <line x1="427" y1="110" x2="427" y2="185"/>
    <line x1="608" y1="110" x2="608" y2="185"/>
    <rect x="65" y="220" width="350" height="120" rx="5"/>
    <rect x="80" y="280" width="150" height="45" rx="4"/>
    <rect x="250" y="280" width="150" height="45" rx="4"/>
    <rect x="440" y="220" width="350" height="120" rx="5"/>
    <line x1="615" y1="275" x2="615" y2="340"/>
    <rect x="65" y="370" width="725" height="70" rx="5"/>
    <line x1="427.5" y1="370" x2="427.5" y2="440"/>
    <rect x="65" y="495" width="725" height="150" rx="8"/>
    <rect x="85" y="550" width="95" height="50" rx="3"/>
    <rect x="180" y="550" width="95" height="50" rx="3"/>
    <rect x="275" y="550" width="95" height="50" rx="3"/>
    <rect x="370" y="550" width="95" height="50" rx="3"/>
    <rect x="465" y="550" width="95" height="50" rx="3"/>
    <rect x="560" y="550" width="95" height="50" rx="3"/>
    <rect x="655" y="550" width="115" height="50" rx="3"/>
  </g>
  <g fill="currentColor" font-family="sans-serif">
    <text x="35" y="43" font-size="18" font-weight="bold">Domain-owned physical backing</text>
    <text x="60" y="88" font-size="17" font-weight="bold">Process region: 1 GiB</text>
    <text x="75" y="138" font-size="14">64 KiB slice</text>
    <text x="256" y="138" font-size="14">64 KiB slice</text>
    <text x="437" y="138" font-size="14">64 KiB slice</text>
    <text x="618" y="138" font-size="14">64 KiB slice</text>
    <text x="65" y="205" font-size="12">A bitmap records which slices are owned.</text>
    <text x="80" y="246" font-size="16" font-weight="bold">Locality segment</text>
    <text x="80" y="267" font-size="12">Consecutive slices owned by one general heap</text>
    <text x="115" y="308" font-size="14">32 KiB slab</text>
    <text x="285" y="308" font-size="14">32 KiB slab</text>
    <text x="455" y="246" font-size="16" font-weight="bold">Medium span</text>
    <text x="455" y="267" font-size="12">One or more consecutive slices</text>
    <text x="500" y="310" font-size="14">slice 1</text>
    <text x="675" y="310" font-size="14">slice 2</text>
    <text x="80" y="397" font-size="16" font-weight="bold">Bump chunk</text>
    <text x="80" y="420" font-size="12">One 64 KiB slice</text>
    <text x="250" y="412" font-size="14">32 KiB segment</text>
    <text x="520" y="412" font-size="14">32 KiB segment</text>
    <text x="860" y="88" font-size="16" font-weight="bold">Direct mapping</text>
    <text x="860" y="118" font-size="12">One large or highly</text>
    <text x="860" y="137" font-size="12">aligned allocation.</text>
    <text x="860" y="170" font-size="12">Managed independently</text>
    <text x="860" y="189" font-size="12">by the operating system.</text>
    <text x="860" y="273" font-size="16" font-weight="bold">Additional region</text>
    <text x="860" y="303" font-size="12">Created when existing</text>
    <text x="860" y="322" font-size="12">regions cannot provide</text>
    <text x="860" y="341" font-size="12">a large enough free run.</text>
    <text x="860" y="374" font-size="12">Uses the same layout.</text>
    <text x="80" y="522" font-size="16" font-weight="bold">A slab contains blocks from one size class</text>
    <text x="110" y="580" font-size="12">header</text>
    <text x="208" y="580" font-size="12">block</text>
    <text x="303" y="580" font-size="12">block</text>
    <text x="398" y="580" font-size="12">block</text>
    <text x="493" y="580" font-size="12">block</text>
    <text x="588" y="580" font-size="12">block</text>
    <text x="690" y="580" font-size="12">...</text>
    <text x="85" y="625" font-size="12">Example: every block in this slab is the selected 64-byte size class.</text>
    <text x="930" y="642" font-size="11">Not to scale</text>
  </g>
</svg>
</div>

Each thread has an implicit general heap. [`with_hint`][__link17] can
temporarily route allocations to an explicit general or bump heap instead;
leaving the scope restores the previous heap. This keeps the common
allocation path thread-local while allowing runtimes and data structures to
choose allocation topology deliberately.

General heaps route requests by size and alignment:

|Category|Size|Maximum alignment|Backing|
|--------|----|-----------------|-------|
|**Small**|Up to 16 KiB|4 KiB|One block in a 32 KiB slab|
|**Medium**|Up to 1 GiB, when no small class fits|64 KiB|One or more 64 KiB slices|
|**Large/direct**|Above 1 GiB, or alignment above 64 KiB|Operating-system limit|Dedicated mapping|

Small frees normally return to the owning heap’s caches; cross-thread frees
are queued for that owner. Medium spans are cached or returned to domain
free lists, while direct mappings go back to the operating system. Dropping
an explicit heap releases empty backing memory but preserves metadata for any
allocations that escaped its lifetime.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/rallocator">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQb2kc6pUCnOEUbs0KFUSLczcMb86XDcWH-dsAbezyKpq8309JhZIKCcGFsbG9jYXRpb25faGludHNlMC4xLjCCanJhbGxvY2F0b3JlMC4xLjA
 [__link0]: https://crates.io/crates/allocation_hints/0.1.0
 [__link1]: https://docs.rs/rallocator/0.1.0/rallocator/?search=Rallocator::new
 [__link10]: https://docs.rs/rallocator/0.1.0/rallocator/macro.rallocator.html
 [__link11]: https://docs.rs/rallocator/0.1.0/rallocator/fn.initialize.html
 [__link12]: https://docs.rs/allocation_hints/0.1.0/allocation_hints/?search=with_hint
 [__link13]: https://docs.rs/allocation_hints/0.1.0/allocation_hints/?search=heap::Heap
 [__link14]: https://docs.rs/rallocator/0.1.0/rallocator/?search=Rallocator::new
 [__link15]: https://docs.rs/allocation_hints/0.1.0/allocation_hints/?search=domain::Domain
 [__link16]: https://docs.rs/allocation_hints/0.1.0/allocation_hints/?search=heap::Heap
 [__link17]: https://docs.rs/allocation_hints/0.1.0/allocation_hints/?search=with_hint
 [__link2]: https://docs.rs/rallocator/0.1.0/rallocator/fn.initialize.html
 [__link3]: https://crates.io/crates/allocation_hints/0.1.0
 [__link4]: https://docs.rs/allocation_hints/0.1.0/allocation_hints/?search=heap::Heap
 [__link5]: https://docs.rs/rallocator/0.1.0/rallocator/?search=config::Config
 [__link6]: https://docs.rs/rallocator/0.1.0/rallocator/macro.rallocator.html
 [__link7]: https://docs.rs/allocation_hints/0.1.0/allocation_hints/?search=domain::Domain
 [__link8]: https://docs.rs/rallocator/0.1.0/rallocator/?search=config::Config
 [__link9]: https://docs.rs/rallocator/0.1.0/rallocator/?search=tunables::Tunables
