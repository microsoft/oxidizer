# Heapwatch — Architecture

How the heap accountant works, and the accuracy contract its mechanism buys.
Implementation-agnostic — for the API see the crate-level rustdoc, for
forward-looking ideas see [`TODO.md`](./TODO.md).

## What heapwatch is

Heapwatch **wraps a global allocator and accounts the volume passing through
it**. Installed as a binary's `#[global_allocator]`, it reports how many bytes
that binary's Rust heap holds and how much has moved through it — continuously,
in production, at a cost low enough to leave on.

It sits among four neighbours:

- Unlike a **heap profiler** (`dhat`, `heaptrack`, jemalloc's profiler), it does
  not attribute bytes to call sites — no backtraces, no per-allocation metadata,
  no shadow map. It answers *how much*, never *who*, which is what buys a
  per-allocation cost of a few adds instead of a stack walk.
- Unlike an **exact accounting wrapper** (`stats_alloc`), it does not pay an
  atomic read-modify-write on a shared cache line per allocation, trading a
  small, bounded, stated inaccuracy for the one cost that scales badly with core
  count.
- Unlike **allocator-native statistics** (jemalloc's `mallctl`, mimalloc's
  stats), it is independent of what is underneath — including the system
  allocator, where no such statistics exist — and counts *requested* bytes, the
  figure a code change moves. Native counters also report retention and
  fragmentation, which heapwatch cannot see; the two are complementary.
- Unlike **OS-level metrics** (RSS, working set, cgroup counters), it isolates
  the Rust heap from stacks, static data, code, and non-Rust allocations.

The design target is a long-running server: a heap in gigabytes, hundreds of
threads, millions of allocations per second, and an operator who wants a gauge
to alert on and a regression signal to bisect.

Four properties hold regardless of workload:

- **Transparency** — the wrapper never changes what the inner allocator does,
  which pointer it returns, or which layout it sees.
- **The recording path is allocation-free and cannot fail** — a fixed number of
  arithmetic operations, never a call back into the allocator, never a panic.
- **The error does not grow with uptime** — it is bounded by the thread count
  and the flush threshold, neither of which depends on how long the process has
  been running.
- **Reading is O(1) in the thread count** — a fixed number of relaxed loads.

Everything else in this document is the price of those four.

## The measurement boundary

The boundary is **successful calls through this `GlobalAlloc`**.

**Inside**: every allocation Rust's heap routes through the registered global
allocator — `Box`, `Vec`, `String`, `Arc`, collection growth, and the same
inside any dependency that uses the standard allocator.

**Outside**: allocations made by native or FFI dependencies that manage their
own memory, even when Rust triggered the work; direct OS calls (`mmap`,
`VirtualAlloc`); anything deliberately routed to `std::alloc::System`, which
bypasses the registered allocator entirely — including Rust's own thread-local
bookkeeping; thread stacks, static data, and the executable image; and the inner
allocator's own metadata, free-list retention, and fragmentation.

**Counted as requested, not as reserved.** `GlobalAlloc` hands the wrapper a
`Layout` — what the caller *asked for* — and offers no hook to ask what the
inner allocator rounded that up to. Each allocation is therefore recorded below
what was actually reserved, by a margin set by the inner allocator's size
classes. This is inherent to the trait, not a shortcut, and it says nothing
about how the total compares to RSS: heap pages requested but never touched, or
swapped out, are not resident at all.

**Demand, not footprint.** Because the wrapper counts what callers asked for,
its numbers are invariant across inner allocators: swapping the system allocator
for mimalloc leaves every counter identical, since the call pattern did not
change. That invariance is deliberate — it is what makes heapwatch the control
variable when something else changes — but it also means heapwatch alone cannot
say whether an allocator swap reduced the memory a process actually holds.
Retention, arena caching, and fragmentation are supply-side quantities, visible
only to the allocator or the OS. Pairing the two is sketched under *Allocator
footprint reporting* in [`TODO.md`](./TODO.md).

## The mechanism

One idea: **accumulate per thread without synchronization, publish in batches**.

```text
        alloc / dealloc / realloc
                    │
                    │  plain, non-atomic adds — no atomics, no shared state
                    ▼
        ┌───────────────────────────────────┐
        │  Pending (this thread only)       │ ◄── reset to zero after a flush
        │  live · bytes · op counts         │
        └───────────────┬───────────────────┘
                        │  churn ≥ threshold ─┐
                        │                     ├─► one batched commit
                        │  thread exit ───────┘
                        ▼
        ┌───────────────────────────────────┐
        │  atomic totals owned by the       │
        │  HeapWatch instance (no shard     │
        │  registry, no aggregation walk)   │
        └───────────────┬───────────────────┘
                        │  a handful of relaxed loads
                        ▼
                     stats()
```

**Churn, not live bytes, triggers the flush.** A thread publishes once *bytes
allocated plus bytes freed* since its last flush reaches the threshold. Churn,
because a thread that allocates and frees in a loop barely moves `live` while
still running the cumulative counters up — and because `|live| ≤ churn`, so
bounding churn bounds both. A thread also flushes when it exits, or live bytes
would drift upward for the life of the process as threads came and went.

### One window, end to end

```text
 alloc(4 KiB) ........ thread block: live +4096, allocated +4096, alloc_ops +1.
                       Nothing global moves — stats() cannot see this yet.
 ~1000 more ops ...... churn accumulates; between operations the residue is
                       always below the threshold. This residue IS the error.
 op crossing 64 KiB .. one batched commit — one fetch_add per counter, live
                       included. The thread block resets to zero and the error
                       returns to zero.
 dealloc(4 KiB) ...... live −4096, freed +4096, free_ops +1
 thread exits ........ the destructor commits whatever remains
 stats() ............. relaxed loads of the committed totals only
```

### Which totals a thread commits to

Totals live on the instance, but the pending block is one thread-local rather
than one per instance, so the block records which counter set it is accumulating
for. Recording compares that target against the instance handling the call; a
mismatch commits the pending values to the previous target and re-targets the
block. A binary has one `#[global_allocator]`, so the compare is a predictable
never-taken branch in practice, and the fallback exists only so that a second
instance cannot silently mix its numbers into the first's.

### Cost

| Path | How often | Work |
|---|---|---|
| Record | every operation | one thread-local address, one target compare, a few non-atomic adds, one compare-and-branch on churn |
| Commit | once per threshold's churn | one `fetch_add` per counter |
| Read | on demand | one relaxed load per field |

At the 64 KiB default and a 64-byte mean allocation, a commit lands roughly once
per thousand operations, so the atomics amortize to a small fraction of the
recording cost — which is the entire point of the threshold.

### Alternatives rejected

Both obvious competitors keep an atomic read-modify-write on the hot path, which
is the cost this design exists to remove:

- **Per-thread shard registry** — a slot claimed at thread start, read by
  walking all slots. Pays registry contention at thread start, a lifetime
  problem for recycled slots, and a read whose cost grows with thread count.
- **Fixed shard array**, indexed per CPU or by thread hash. The shard count is
  fixed and needs no destructor, but stable Rust has no cheap, migration-safe
  per-CPU index — no restartable sequences, and `sched_getcpu` is a syscall — so
  the index comes from a thread-local anyway. That pays the atomic *and* the
  thread-local read, and suffers false sharing when hot threads collide.

### Ordering, signedness, and arithmetic

**Relaxed ordering is sufficient.** The totals carry no data — they *are* the
data. Nothing is published through them and no reader needs a happens-before
relationship with the writer. Stronger ordering would not buy a coherent
snapshot either, since the fields are read independently.

**Live bytes are signed.** A thread that frees memory another thread allocated
accumulates a negative `live`, and if it flushes first the instance's total goes
briefly negative. Unsigned modular arithmetic would recover once the matching
allocation was published, but meanwhile every reader would see a value near
`u64::MAX`. The derived net allocation count is signed for the same reason:
batching can publish frees before their allocations.

**All accounting arithmetic is explicitly wrapping or saturating and cannot
panic.** Unwinding out of `GlobalAlloc` is undefined behavior, so an overflow
check that panics in a debug build is not an acceptable failure mode. `Layout`
guarantees a size no greater than `isize::MAX`, which makes the width conversion
into signed accounting exact on targets up to 64 bits; the `|live| ≤ churn`
argument holds under those semantics.

## Public surface

Counters belong to the **instance**, not to a set of process-wide statics. A
`HeapWatch` owns its totals and hands out a handle to read them, so the crate
carries no hidden global state and two instances count independently.

Registration is a `const` constructor in a `static`, as `#[global_allocator]`
requires. The flush threshold is a const generic — so it stays a compile-time
constant on the hot path — and defaults to 64 KiB:

```rust
use heapwatch::HeapWatch;
use std::alloc::System;

#[global_allocator]
static HEAP: HeapWatch<System> = HeapWatch::new(System, "system");

// Or with an explicit threshold:
// static HEAP: HeapWatch<System, { 8 * 1024 }> = HeapWatch::new(System, "system");
```

Reading goes through `Watch`, a handle with no type parameters, so a diagnostic
endpoint can report the heap without the allocator's generics being threaded
through to it:

```rust
fn report(heap: heapwatch::Watch) {
    let s = heap.stats();
    println!(
        "{} live, {} allocated, {} freed, {} net allocations",
        s.current_bytes,
        s.allocated_bytes,
        s.freed_bytes,
        s.net_alloc_ops(),
    );
}

fn main() {
    report(HEAP.watch());
}
```

The rest of the surface:

- **`HeapWatch::new(inner, name)`** — `const`, generic over the inner allocator
  and the threshold. `name()` returns the stored name: heapwatch emits no
  telemetry itself, so whatever does emit can tag the series with it, making a
  before/after comparison across an allocator change a dashboard filter rather
  than a reconstruction from deployment timestamps.
- **`watch()`** — a `Copy + Send + Sync` handle valid for the life of the
  process. `stats()` is callable on the instance and on the handle alike.
- **`stats()`** — a plain `Copy` snapshot: current bytes, cumulative bytes
  allocated and freed, allocation, free, and reallocation counts, plus the
  derived net allocation count. It is `#[non_exhaustive]`, so adding a counter
  later stays additive.

**There is deliberately no flush operation.** Committing is entirely internal —
the churn threshold and thread exit — so a caller has one verb, *read*, and
nothing to sequence before it. Exposing a flush would offer callers a way to
tighten a bound that is already documented and small, in exchange for an API
whose correct use is subtle: it only affects the calling thread, so tightening
a process-wide reading would mean calling it on every thread, which is not
something a diagnostic endpoint can do.

**Concurrency contract.** Every entry point is callable from any thread at any
time; none blocks, none allocates, and none can fail. Concurrent readers and
writers race in the ordinary way; since the counters are a gauge rather than a
protocol, that is not a correctness problem, only the imprecision the accuracy
section bounds.

Heapwatch requires `std`, a deliberate exception to the workspace's `no_std`
preference: the mechanism needs a thread-local with a thread-exit destructor,
and `thread_local!` is the stable way to get one. It also requires 64-bit
atomics, which excludes targets without `target_has_atomic = "64"`.

## Accuracy

The proposition is not that the numbers are exact, but that the error is
**bounded, enumerated, and small relative to the decisions they inform**.

| Source | Direction | Bound |
|---|---|---|
| Unflushed pending values | either, self-correcting | `threshold × live threads` |
| Requested vs. reserved size | under | inner allocator's size-class rounding |
| Non-`GlobalAlloc` allocations | under | outside the stated scope |

`stats()` omits whatever each live thread has not yet flushed. The threshold
test runs *after* an operation is applied, so a single large allocation can push
a window far past the threshold — but it flushes immediately when it does, so
between operations a thread's residue is always below the threshold, and
`|live| ≤ churn` carries that to live bytes. Hence the bound, which holds at
quiescent operation boundaries: a reader racing an allocation in flight on
another thread can miss that whole allocation, however large. At the 64 KiB
default and a few hundred threads it is tens of megabytes against a heap in
gigabytes.

Two caveats. The bound does not *converge*: a thread that parks below the
threshold hides its residue for as long as it sleeps, so a large idle pool can
hold the full bound indefinitely, and with no flush operation there is nothing a
reader can do about it. And it is stated in live threads, a quantity `stats()`
does not report, so a caller wanting a numeric error bar must supply the
thread-count ceiling from its own deployment knowledge.

Lowering the threshold tightens the bound and raises the per-allocation share of
the atomic cost; raising it does the reverse. Zero flushes every operation:
exact accounting with the cost profile of a conventional atomic-per-allocation
tracker — useful in tests, not in production.

**Snapshot consistency is not claimed.** Reading is a sequence of independent
relaxed loads, not a stop-the-world snapshot, so fields of one reading may be
mutually inconsistent by a few operations. Treat each as a gauge; do not solve
equations across them.

## Re-entrancy

The accounting path runs **inside the allocator**, so anything it does that
itself allocates re-enters `alloc` unboundedly. The design's answer is blunt:
**nothing on the accounting path may allocate, ever** — no formatting, no
collections, no boxing error types, no logging. That is a standing invariant on
the implementation, not a property of the current code.

The one step that looks like it might allocate is the thread-local access, since
the pending block carries the exit-flush destructor and a thread-local with a
destructor must register it on first touch. It does not re-enter, because the
standard library deliberately backs *all* of its thread-local machinery with
`std::alloc::System` rather than the registered global allocator — the
destructor list, the boxed value on targets without native thread-locals (whose
source comment cites exactly this hazard), and the linux-like path's use of
libc's `__cxa_thread_atexit_impl`. First touch may therefore allocate, but never
through the allocator being measured.

Consequently the design carries **no re-entrancy guard flag**, which would cost
a branch and a second thread-local access on every operation to defend a case
that cannot arise. This rests on a standard-library implementation choice rather
than a documented guarantee, so the compensating control is a test that installs
heapwatch as the real global allocator and forces first-touch initialization
from inside it: a regression fails CI loudly instead of degrading silently.

One fallback is genuinely necessary. Once a thread's pending block has been
destroyed, further allocations on that thread — during the tail of thread
shutdown, as other thread-locals tear down — find it gone. Those events **commit
a one-off batch straight to the instance's atomics**. Dropping them would be
worse than imprecise: the matching frees would still be counted, skewing live
bytes downward a little further for every thread the process ever started.

## Operation counting

Every `GlobalAlloc` entry point maps to counters by one rule — bytes follow the
*change* in the block's size, counts follow whether a logical block came or went:

| Call | Live bytes | Cumulative bytes | Count |
|---|---|---|---|
| `alloc`, `alloc_zeroed` | `+size` | allocated `+size` | `alloc_ops` |
| `dealloc` | `−size` | freed `+size` | `free_ops` |
| `realloc` grown | `+delta` | allocated `+delta` | `realloc_ops` |
| `realloc` shrunk | `−delta` | freed `+delta` | `realloc_ops` |
| any null return | — | — | — |

Reallocations are counted separately because a reallocation is the same logical
block at a new size, not a new block: folding them into `alloc_ops` would make
every growing `Vec` look like a leak. Failed allocations moved no bytes, and
counting them would drift the count upward under exactly the pressure where a
correct leak signal matters most.

That separation is what makes `alloc_ops − free_ops` meaningful as the **net
successful allocation count**: roughly flat at steady state regardless of
traffic, climbing under a leak of whole blocks. It is often the earlier signal,
because many small leaked allocations move the count long before they move a
byte total dominated by a few large buffers. It is a batched estimate rather
than a census, can go transiently negative, and does not detect growth that
comes purely from reallocating existing blocks larger.

## Transparency of the wrapper

Pointers and layouts pass through unchanged — no layout adjustment for metadata
and no per-allocation header, which is also why `dealloc` can rely on the
caller's `Layout` rather than a shadow map. `realloc` and `alloc_zeroed` are
forwarded rather than left to their `GlobalAlloc` defaults, which would
respectively discard the inner allocator's in-place growth and forfeit pages the
OS already zeroed — regressions far costlier than the tracking itself. The inner
allocator needs nothing beyond `GlobalAlloc` and is reachable by reference, so
one that exposes its own controls stays usable.

## Edge cases and limits

- **Thread exit is best-effort.** The standard library does not guarantee
  thread-local destructors run — notably not for the main thread at process
  exit, nor after `process::exit`. A thread whose destructor is skipped loses
  its residue, which is bounded by the threshold, so the accuracy contract
  survives; the "exactly once" property does not.
- **Signal handlers.** Recording is a non-atomic read-modify-write of the
  thread's own block, so an allocating signal handler interleaved with it can
  lose an update. Allocation in a signal handler is already unsound — the inner
  allocator is generally not async-signal-safe — so this is declared
  unsupported rather than defended.
- **`fork()` without `exec`.** The child inherits the committed totals, the
  parent's whole allocation history, and only the forking thread. Every other
  thread vanishes without flushing. Nothing re-initializes; fork-without-exec is
  unsupported.
- **Nesting.** `HeapWatch<HeapWatch<_>>` records every event into both
  instances' counters. The type system does not prevent it; it is unsupported.
- **Zero sizes and OOM.** `GlobalAlloc` already requires a non-zero layout and a
  non-zero `realloc` size, so zero-sized operations never reach the wrapper and
  "shrink to zero" is not a valid reallocation. Heapwatch neither installs nor
  intercepts an allocation-error hook; an aborting OOM leaves pending values
  unpublished.
- **Counter widths.** Cumulative byte totals are 64-bit and would need centuries
  at gigabytes per second to wrap; live bytes are signed 64-bit, beyond the
  reach of any real heap.

## Design invariants at a glance

1. **The accounting path never allocates**, and never panics. Every step is
   wrapping or saturating arithmetic on a thread-local block, or a relaxed
   atomic on the instance's counters.
2. **Re-entrancy cannot occur**, because thread-local machinery is backed by
   `System` rather than the registered allocator, and post-teardown events take
   the direct-to-totals path.
3. **Every recorded event reaches the totals at most once, and exactly once**
   for normal thread termination outside a signal handler; what a skipped
   destructor loses is bounded by the threshold.
4. **A thread's residue is below the threshold between operations**, so the
   reporting error is bounded and does not grow with uptime.
5. **Reading is O(1) in the thread count.** No registry, no walk, no iteration.
6. **The wrapper is transparent.** Pointers, layouts, and the `realloc` and
   `alloc_zeroed` specializations pass through unchanged.
7. **Bytes are counted as requested**, so each recorded allocation is a lower
   bound on what the inner allocator reserved for it.
