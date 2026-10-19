# Windows performance-counter design

## Metric behavior

The processor connects typed `observed` events to local Windows performance
counters. Startup reads the event's static field inventory and binds instruments
to stable numeric field indices. Emission uses the existing lazy field visitor:
it enumerates routed fields but evaluates only bound numeric getters, without
resolving metric names or visiting enrichment. Optional fields keep their indices
even when their visits are omitted. Fieldless counters record one separately.

Unsigned counters add, signed up/down counters adjust a nonnegative total, and
unsigned gauges replace their readings. Arithmetic overflow and underflow leave
the previous value intact. The first processing failure is logged and retained
for `Sink::flush()`; later valid updates can still succeed. A selected counter
set contains between one and 256 counters with unique display names.

Sink emission follows the sink's timestamp, context, and sampling rules. Use an
unsampled sink for deltas. Numeric recording does not extract enrichment, redact
values, or create general signal records.

## Deployment and runtime

The machine-wide Windows catalog describes names, IDs, types, and help text.
The externally authored manifest and its compiled resource DLL populate that catalog
through an elevated installer. Runtime registration creates process-local
instances with live readings; it does not install definitions.

The supplied `.man` is authoritative for provider/set GUIDs, counter IDs, and
native definitions. `category` selects a declared set by name; it does not
derive deployment identities. Different processes can expose those definitions
through different instances. Definition changes require coordinated deployment.
The application reads the same file once through `manifest_file`.
Every bound instrument name must exactly match a declared counter, with no
duplicate destination bindings. Additional declared counters stay at zero.
The [API documentation source](../src/lib.rs) contains the usage and installation
instructions. A deployment collection service, such as the one configured for
PerfSpy, remains separate from local publication.

## Publication safety boundary

Each registered reference points directly at an `AtomicU64` emission total.
No ordinary `u64` publication cells or collection-time copies exist.
The backing allocation stays at a stable address until native shutdown succeeds.

The FFI access contract combines these guarantees:

- [`PerfSetCounterRefValue`][reference] specifies a pointer to a four- or
  eight-byte unsigned integer which Perflib dereferences during collection.
  This backend accepts only eight-byte `perf_counter_large_rawcount` references.
- [64-bit Windows guarantees atomic reads of aligned 64-bit variables][windows].
  This backend does not publish counters on 32-bit Windows.
- [`AtomicU64`][atomic] has `u64`'s size and bit validity, with eight-byte
  alignment. Rust writes use only full-width atomic operations, and native
  collection reads the full-width integer. No other payload depends on these
  reads, so relaxed memory ordering is sufficient.

These are Windows scalar-access guarantees, not permission to mix Rust atomic
writes with ordinary Rust pointer reads. The injected native API requires
read-only, full-width atomic access; mock consumers therefore use atomic loads.
Partial-width access, concurrent native writes, and publishing other storage
types are outside this contract.

This follows [.NET's V2 `CounterData` reference-publication pattern][dotnet],
which registers stable unmanaged integers and updates them with `Interlocked`
operations rather than freezing collection. The .NET implementation is
supporting precedent; it does not replace the platform access contract above.

The [Perflib callback deadline][callback] is not a synchronization primitive.
The callback is a no-op, so a delayed, overlapping, or unmatched START/END
notification cannot write stale snapshots or release a nonexistent reader gate.
Collection may observe an older or newer value of each counter, but does not
obtain a transactional snapshot across counters.

See the [implementation guide](implementation.md) for ownership and validation.

[callback]: https://learn.microsoft.com/windows/win32/api/perflib/nc-perflib-perflibrequest
[reference]: https://learn.microsoft.com/windows/win32/api/perflib/nf-perflib-perfsetcounterrefvalue
[windows]: https://learn.microsoft.com/windows/win32/sync/interlocked-variable-access
[atomic]: https://doc.rust-lang.org/1.95.0/std/sync/atomic/struct.AtomicU64.html
[dotnet]: https://github.com/dotnet/runtime/blob/81cabf2857a01351e5ab578947c7403a5b128ad1/src/libraries/System.Diagnostics.PerformanceCounter/src/System/Diagnostics/PerformanceData/CounterSetInstanceCounterDataSet.cs
