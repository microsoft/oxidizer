# Heapwatch — Unimplemented Ideas & Future Work

Designs and ideas that are **not** part of the shipped architecture, which is
documented in [`DESIGN.md`](./DESIGN.md). Items here range from small
follow-ups to features that would change the crate's character.

## 1. Peak live bytes

The counters report the heap as it is now, not the highest it has been, so a
spike shorter than the emission interval leaves no trace. Tracking a peak
per thread — the running maximum of local live, folded into a process-wide
`fetch_max` at each commit — would catch those spikes at the cost of one
compare per allocation and one atomic per commit.

Deliberately not done for now: a consumer that already samples live bytes can
maintain its own maximum, and every other view of one — arbitrary windows, fleet
aggregation, correlation — is better derived downstream anyway. Doing it in the
crate also drags in a reset operation and a subtler contract, because a
candidate combines a local maximum measured at one instant with a total read at
a later one, so a monotone maximum acquires a small *persistent* upward bias
where the other counters carry only a transient one.

## 2. A reported error bar

The accuracy bound is `threshold × live threads`, and `stats()` reports neither
factor — so a caller cannot turn the documented bound into a number. Tracking a
live-thread count would cost one atomic increment at thread start and one
decrement at exit, nothing on the allocation path, and would let the snapshot
carry a computed `error_bound_bytes`.

Deliberately not done for now: it adds a counter that most consumers would
ignore, and an operator who needs the figure already knows their deployment's
thread-count ceiling. Worth revisiting if the crate grows a consumer that has to
decide programmatically whether a reading is trustworthy.

## 3. Telemetry integration

Today the crate reports numbers and nothing more; emitting them is the caller's
job. A thin optional layer that registers the gauges with a metrics backend
would remove the boilerplate every adopter otherwise writes — periodic emission
of live bytes and the net allocation count, with the allocator name as a
dimension.

The reason to keep it optional and separate is dependency weight: the core crate
should stay a `GlobalAlloc` wrapper with no metrics dependency, so that a binary
that already has its own emission path pays nothing.

## 4. Usable-size accounting

Byte figures are recorded as *requested*, because `GlobalAlloc` has no hook to
ask the inner allocator what it actually reserved. Allocators generally do know
— jemalloc exposes `nallocx`/`malloc_usable_size`, mimalloc exposes
`mi_usable_size` — so an optional trait that an inner allocator could implement
to report the rounded size would close the largest systematic error in the
accuracy table.

The obstacle is that the answer is only useful if it is cheap: a per-allocation
size-class lookup on the hot path would trade a documented, stable
under-estimate for real overhead. Worth exploring only for allocators whose
rounding can be computed arithmetically from the layout.

## 5. Scoped or per-category accounting

The counters cover the whole process, which answers "how big is the heap" but not
"which subsystem grew". A scoped variant — a guard that attributes allocations
on the current thread to a named category for the duration of a request or task
— would give per-category totals without call-site attribution.

This is a significant change in character, not an increment: it needs a
thread-local category stack, a set of counters per category, and a policy for
allocations that outlive the scope that created them (the common case, and the
hard part). Freed-elsewhere allocations would be attributed to whoever frees
them unless a shadow map recorded ownership, which is the cost the whole design
avoids.

## 6. Sampling-based call-site attribution

A sampled backtrace on one allocation in every N would recover a coarse version
of what a heap profiler provides, at a cost proportional to the sampling rate
rather than to the allocation rate. It fits the crate's philosophy — bounded,
stated inaccuracy in exchange for affordable continuous operation.

It also conflicts with the crate's simplest promise, that the recording path
never allocates: capturing and symbolizing a backtrace does. A viable version
would have to buffer raw frame pointers into fixed-size storage and resolve them
off the allocation path.

## 7. Allocation-size histogram

A small fixed set of size-class buckets, incremented on the same thread-local
block and flushed with everything else, would show whether a heap grew because
of more allocations or larger ones — a question the current counters can only
answer by division. The cost is one extra branch and increment on the record
path, plus a wider commit; whether that is worth it depends on how often the
distinction actually drives a decision.

## 8. Allocator footprint reporting

Counters record what callers *requested*, so they are invariant across inner
allocators by construction: swapping the system allocator for mimalloc leaves
every number identical, because the call pattern did not change. What does
change is the memory the allocator holds from the OS — size-class rounding,
free-list retention, arena caching, fragmentation, and metadata — and none of
that is reachable through `GlobalAlloc`.

Most production allocators do report it: mimalloc's `mi_process_info` yields
current and peak committed bytes, and jemalloc's `mallctl` exposes
`stats.active`, `stats.resident`, `stats.mapped`, and `stats.retained`. An
optional trait that an inner allocator could implement — one method returning
committed bytes, called on demand rather than per allocation — would let
heapwatch report demand and footprint side by side. The ratio between them is
the figure that actually moves when the allocator changes, and it is also the
per-component attribution the OS cannot give: an allocator instance linked into
one library measures that library's heap, not the whole process.

The reasons to keep it optional and off the recording path: the figures come
from the allocator's own bookkeeping, whose cost and freshness vary (jemalloc
needs an epoch advance to refresh, mimalloc merges thread-local stats), and no
equivalent counter exists for the system allocator — so the trait has to be
implementable only where it means something.
