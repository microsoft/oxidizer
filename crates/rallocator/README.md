<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Rallocator Logo" width="96">

# Rallocator

[![crate.io](https://img.shields.io/crates/v/rallocator.svg)](https://crates.io/crates/rallocator)
[![docs.rs](https://docs.rs/rallocator/badge.svg)](https://docs.rs/rallocator)
[![MSRV](https://img.shields.io/crates/msrv/rallocator)](https://crates.io/crates/rallocator)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

A pure-Rust, high-performance allocator with scoped heaps and telemetry.

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
```

The macro declares the required `#[global_allocator]` static and contains
the unsafe call to [`Rallocator::new`][__link0], whose process-wide
same-configuration invariant it establishes by construction.

Ordinary allocations then use each thread’s implicit general heap. To group
related allocations, use an [`allocation_hints`][__link1] prospective heap. These
handles publish only a thread-local request. Rallocator lazily realizes the
request and retains a bounded per-thread cache for later reattachment:

```rust
use allocation_hints::heaps::{Heap, bump};
use allocation_hints::with_hint;

rallocator::rallocator!();

let heap = Heap::bump(bump::Options::new());
let values = with_hint(&heap, || vec![1, 2, 3]);
assert_eq!(values.len(), 3);
```

If another global allocator is installed, the same code remains valid and
the hint may be ignored.

### Reallocation

Both global-allocator entry points can resize an untracked ordinary small
block within its actual size class on the current owning heap, or an
untracked medium block within its existing physical span. Medium resizing
also works after transfer or owner exit, without moving the allocation to
the current hint’s heap. Small blocks on foreign or retired heaps, remote
slabs, context/tracked blocks, bump blocks, direct mappings and snapshot
storage retain allocate-copy-free behavior. An unchanged size is a no-op.

In-place resizing preserves alignment and ownership. Requested-byte totals
include growth as allocated bytes and shrinkage as deallocated bytes, while
object counts and allocation/free events do not change. The final free uses
the new requested size. Existing untracked objects do not acquire recording
identities on resize; already tracked objects use fallback even if recording
has since stopped. Checked backing-size failures or replacement allocation
failure leave the original allocation and contents intact. The unsafe
[`std::alloc::GlobalAlloc::realloc`][__link2] caller contract still requires a nonzero
new size whose aligned layout fits `isize::MAX`; internal defensive checks
do not make invalid trait calls valid.

### Telemetry

Snapshot collection uses independently owned system-allocator storage, excluded
from allocator counters and events. Source caches, returned errors and panic
payloads remain valid after capture and can be released on other threads.
These diagnostic allocations still contribute to process memory use, but not
rallocator’s mapped-byte or live-allocation totals.

Telemetry is opt-in at compile time through [`rallocator!`][__link3]:

```rust
use seismograph::recorder::{Configuration, RecordingPolicy};

rallocator::rallocator!();

fn main() -> Result<(), Box<dyn std::error::Error>> {
    seismograph::recorder(Configuration {
        allocations: RecordingPolicy::all(true),
        ..Default::default()
    });
    let mut values = vec![1, 2, 3];
    values[0] += 1;
    std::hint::black_box(&values);
    drop(values);
    seismograph::recorder(Configuration::default());

    seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default())
        .expect("telemetry snapshot capture succeeds")
        .write_file("snapshot.seismograph")?;

    Ok(())
}
```

Convert the snapshot to HTML with `seismograph snapshot html snapshot.seismograph`.

Process-wide byte and operation counters remain active for the process
lifetime. Threads publish these counters in batches of 256 operations;
snapshots flush the capturing thread, while other active threads can lag by
at most one batch. Per-size histograms, allocation events, and backtraces
require Seismograph recording, which can be enabled only around the interval
of interest to limit its overhead.

Cumulative remote-free and remote-drain counts are updated immediately in
64 fixed atomic shards, not buffered for 64 events. Reading both counts folds
128 relaxed loads over an observation interval: concurrent values can be
stale or combine shard histories, rather than describe one common instant.
After writers synchronize and stop, totals are exact modulo `usize`.
Pending and in-progress remote gauges remain scalar atomics. Normal slab
drains logically claim the entire detached list before recycling any node;
retirement still claims nodes individually. A drain count or zero pending
count therefore does not prove physical recycling, reclamation or quiescence.
These lifetime counters are independent of opt-in recording sessions.
The default `caller-symbolization` feature resolves captured instruction
pointers through the optional `backtrace` dependency. Disabling default
features retains caller tracking and raw addresses without in-process symbol
resolution.

## Design guide

Use the implicit thread-local general heap for ordinary allocations.
Introduce prospective general heaps when you want locality boundaries without
changing the mixed-size allocation model. Use bump heaps for phase-bounded
work where individual frees are rare and bulk reclamation matters more than
per-allocation reuse. Use [`allocation_hints::heaps::thread_heap`][__link4] when another
thread should allocate into the current thread’s allocator-preferred heap.

## Implementation guide

1. Install and configure [`rallocator!`][__link5] exactly once as the process-global
   allocator.
1. Route special-purpose allocations with [`allocation_hints::with_hint`][__link6]
   and [`allocation_hints::heaps::Heap`][__link7].
1. Enable Seismograph recording only when you need allocation events and
   backtraces; lifetime aggregate counters remain active.

Invalid tunables fail early when [`Rallocator::new`][__link8] is instantiated: the
size-class layout must be well-formed and the partial-slab scan limit must
be non-zero.

## Internals

Rallocator is organized as a hierarchy:

* An internal allocation domain owns one or more 1 GiB virtual-memory
  **regions**, divided into 64 KiB **slices**.
* A domain can serve multiple allocator-native heap realizations.
  Each heap keeps its own allocation state while drawing backing memory from
  its domain.
* General heaps use slices for **locality segments** containing 32 KiB
  **slabs**, and for **medium spans** covering one or more slices. Bump heaps
  use slices as **bump chunks**.
* Slabs contain same-sized **blocks**, which are the allocation slots returned
  for small requests. Large or highly aligned requests bypass this hierarchy
  and use direct operating-system mappings.

### Medium allocation locality and reclamation

A logical domain has sixteen fixed backing shards. Medium heaps choose a
shard on first use from the current NUMA node, without dynamic topology
allocation or hard memory binding. A per-bucket ticket balances heaps over
four contention lanes, including when unpinned workers start on the same CPU.
Machines with more than
four nodes share buckets. Selection stays stable for a heap: migrating threads
remain correct but may lose locality. Small slabs and bump chunks continue to
use the primary domain shard.

Medium allocations refill heap-local batches of fresh or recycled spans.
Refills grow from one to sixteen spans with demand; all cached classes
together retain at most 1 MiB per heap. Batching applies to locally eligible
power-of-two slice counts; other medium sizes use shared backing directly.
A fresh refill reserves and commits a contiguous extent in one transaction,
then serves its remaining spans without
taking a shared allocator lock. Allocation sizes and 64 KiB rounding are
unchanged.
When a local cache fills, its contents return to the backing shard in one
bounded batch, preserving each span’s class. The incoming free stays local;
the combined 1 MiB cache budget does not increase.

Cross-thread frees enter the allocation’s backing-shard cache immediately,
not a queue that its allocating thread must eventually drain. Any heap using
that shard can take a batch. The existing heap retirement protocol protects
allocation ownership metadata; shared free spans do not retain heap pointers.
Shared backing retains recently observed demand through the purge-delay
window; expired caches target a 16 MiB idle floor. One allocator-wide pool
grants demand-sized retention credits in 8 MiB units, rather than reserving
equal allowances for inactive shards. Credits survive short reuse cycles and
are returned as demand and retained backing shrink. The pool limit is existing
grants plus half the available headroom, capped at half of effective memory;
low available headroom targets zero retention. The limit is capacity for
observed demand, not a preallocated cache.
Pressure hints use allocation-free OS queries, including best-effort standard
cgroup-v2 limits on Linux. Custom cgroup mounts and cgroup-v1 limits are not
currently discovered. These targets are not strict RSS limits.
Concurrent frees, bounded maintenance, and OS failures can temporarily exceed
them.

Reclamation detaches up to 64 spans / 4 MiB per maintenance packet (or one
larger span), then combines adjacent ranges from the same region before
decommit. Heap retirement coalesces its local batches too. Periodic local
medium cache hits rotate maintenance across the domain’s shards. A completely
idle process retains cached backing until subsequent medium activity; there
is no background maintenance thread.

Region availability and bitmap-word scans bound normal refill searches.
Commit, decommit, and region mapping run outside allocator spin locks, with
reserved bitmap bits protecting in-flight operations. Opportunistic purge
does bounded work; OS failures may retain or quarantine spans rather than
exposing inaccessible memory. These policies need workload-specific
throughput and retention measurements; they are not a production-readiness
guarantee.

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

Each thread has an implicit general heap. [`allocation_hints::with_hint`][__link9]
can temporarily route allocations to a prospective general, bump, or
thread-target heap; leaving the scope restores the previous request. This
keeps the common allocation path thread-local while allowing runtimes and
data structures to choose allocation topology deliberately.

General heaps route requests by size and alignment:

|Category|Size|Maximum alignment|Backing|
|--------|----|-----------------|-------|
|**Small**|Up to 16 KiB|4 KiB|One block in a 32 KiB slab|
|**Medium**|Up to 1 GiB, when no small class fits|64 KiB|One or more 64 KiB slices|
|**Large/direct**|Above 1 GiB, or alignment above 64 KiB|Operating-system limit|Dedicated mapping|

Small frees normally return to the owning heap’s caches; cross-thread frees
are queued for that owner. Medium spans are cached or returned to domain
free lists, while direct mappings go back to the operating system. Detached
prospective realizations remain in a bounded per-thread cache; eviction
releases empty backing while preserving metadata needed by escaped
allocations.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/rallocator">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQb3vQhumeffeMbtCBsjb4ytk4bRX3c-neOHh0b-7x_5VEQkTthZIKCcGFsbG9jYXRpb25faGludHNlMC4xLjCCanJhbGxvY2F0b3JlMC4xLjA
 [__link0]: https://docs.rs/rallocator/0.1.0/rallocator/?search=Rallocator::new
 [__link1]: https://crates.io/crates/allocation_hints/0.1.0
 [__link2]: https://doc.rust-lang.org/stable/std/?search=alloc::GlobalAlloc::realloc
 [__link3]: https://docs.rs/rallocator/0.1.0/rallocator/macro.rallocator.html
 [__link4]: https://docs.rs/allocation_hints/0.1.0/allocation_hints/?search=heaps::thread_heap
 [__link5]: https://docs.rs/rallocator/0.1.0/rallocator/macro.rallocator.html
 [__link6]: https://docs.rs/allocation_hints/0.1.0/allocation_hints/?search=with_hint
 [__link7]: https://docs.rs/allocation_hints/0.1.0/allocation_hints/?search=heaps::Heap
 [__link8]: https://docs.rs/rallocator/0.1.0/rallocator/?search=Rallocator::new
 [__link9]: https://docs.rs/allocation_hints/0.1.0/allocation_hints/?search=with_hint
