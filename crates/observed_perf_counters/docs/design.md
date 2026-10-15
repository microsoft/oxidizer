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
