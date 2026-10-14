# Windows performance-counter design

## Metric behavior

The processor connects typed `observed` events to local Windows performance
counters. Startup binds every instrument on an event to a numeric slot.
Emission reads those slots without resolving metric names or visiting general
telemetry fields.

Unsigned counters add, signed up/down counters adjust a nonnegative total, and
unsigned gauges replace their readings. Arithmetic overflow and underflow leave
the previous value intact. The first processing failure is logged and retained
for `Sink::flush()`; later valid updates can still succeed. A category contains
between one and 256 instruments with unique display names.

Sink emission follows the sink's timestamp, context, and sampling rules. Use an
unsampled sink for deltas. Numeric recording does not extract enrichment, redact
values, or create general signal records.

## Deployment and runtime

The machine-wide Windows catalog describes names, IDs, types, and help text.
The generated manifest and its compiled resource DLL populate that catalog
through an elevated installer. Runtime registration creates process-local
instances with live readings; it does not install definitions.

Schema identity depends on category and instrument definitions, not instance
name or binding order. Different processes can therefore expose the same schema
through different instances. Definition changes require coordinated deployment.
The [API documentation source](../src/lib.rs) contains the usage and installation
instructions. A deployment collection service, such as the one configured for
PerfSpy, remains separate from local publication.

## Publication safety boundary

Emission totals are atomic and separate from the cells registered with Perflib.
The collection protocol refreshes those cells on START and freezes them while
admitted collections are active.

**That admission protocol does not establish safety for a timed-out START.**
The [Perflib callback contract][callback] allows collection to proceed before a
late callback returns. A thread can remain descheduled during publication, so a
bound on copying work is not a synchronization guarantee.

Native publication needs an access or lifetime contract that also covers this
case before the registered mutable-cell design is suitable for release.
Replacing writes with a convenience setter alone does not establish such a
contract. See the [implementation guide](implementation.md) for the ownership
and coordination involved.

[callback]: https://learn.microsoft.com/windows/win32/api/perflib/nc-perflib-perflibrequest
