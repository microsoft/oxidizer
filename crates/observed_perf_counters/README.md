<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Observed Perf Counters Logo" width="96">

# Observed Perf Counters

[![crate.io](https://img.shields.io/crates/v/observed_perf_counters.svg)](https://crates.io/crates/observed_perf_counters)
[![docs.rs](https://docs.rs/observed_perf_counters/badge.svg)](https://docs.rs/observed_perf_counters)
[![MSRV](https://img.shields.io/crates/msrv/observed_perf_counters)](https://crates.io/crates/observed_perf_counters)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Publish `observed` integer metrics as Windows performance counters.

Windows keeps a machine-wide catalog mapping counter names to IDs and numeric
formats; applications hold the live readings in memory. At startup, `build()`
registers the matching IDs and memory addresses, letting monitoring tools find
a counter by name and ask Windows for its reading. Multiple application
processes can publish the same counters, each through a named instance.

## Native publication limitation

The reference-cell protocol does not yet establish safe concurrent access
when Windows proceeds after a collection-start callback times out. Native
publication is not ready for deployment until that access contract is
established or the storage design is replaced. See the
[publication safety boundary][__link0].

## Usage

```rust
use std::process::id;

use observed::{Sink, emit, event};
use observed_perf_counters::PerfCounterProcessor;
use performables::arc::Arc;
use tick::SimpleClock;

#[event("request.completed")]
#[counter(bytes_sent, name = "Bytes Sent", unit = "By")]
struct RequestCompleted {
    #[unredacted]
    bytes_sent: u32,
}

let builder = PerfCounterProcessor::builder()
    .category("MyService")
    .instance(format!("worker-{}", id()))
    .bind_event::<RequestCompleted>();

// Deployment tooling: save these definitions as MyServiceCounters.man.
let xml = builder.manifest("MyServiceCounters.dll")?;

// Application startup: create this process's counter instance.
let processor = Arc::into_std_arc(Arc::new(builder.build()?));
let sink = Sink::new("app", vec![processor], SimpleClock::new_system());
emit!(sink, RequestCompleted { bytes_sent: 4096 });
```

`bind_event` registers all instruments on the event using their declared
names, units, and descriptions. Names must be unique within the category.

|Instrument|Value|Meaning|
|----------|-----|-------|
|`counter`|Unsigned integer, or no field|Add to the total; no field adds one.|
|`updown_counter`|Signed integer|Adjust the total, which cannot fall below zero.|
|`gauge`|Unsigned integer|Replace the previous reading.|

Histograms, floats, and signed gauges are unsupported. Overflow or underflow
leaves that metric unchanged, logs the first error through `tracing`, and
returns it from an explicit `Sink::flush()`. Other valid updates continue.
`flush` does not trigger Windows collection. Use an unsampled sink for deltas.

## Deployment

**Register the definitions with Windows.** Windows cannot infer counters from
your Rust events. `builder.manifest("MyServiceCounters.dll")` generates XML
mapping their names to stable IDs, numeric formats, and descriptions.
Save it as `MyServiceCounters.man`; the Windows SDK compiles its names and
descriptions into a resource DLL:

Run these commands in a Windows SDK Developer Command Prompt:

```text
ctrpp -o MyServiceCounters.h -rc MyServiceCounters.rc MyServiceCounters.man
rc /fo MyServiceCounters.res MyServiceCounters.rc
link /dll /noentry /machine:x64 /out:MyServiceCounters.dll /implib:MyServiceCounters.lib MyServiceCounters.res
```

Ship the `.man` and DLL with your application. An elevated installer runs
`lodctr /m:"C:\Program Files\MyService\MyServiceCounters.man"` to add these
definitions to Windows’ catalog. Register them once on each computer, not
for each running process; repeat when your declared metrics change.
The DLL contains descriptive text, not Rust code or live counter values.
For ARM64, replace `/machine:x64` with `/machine:arm64`.

**Publish from each process.** On 64-bit Windows, `build()` registers the IDs,
numeric formats, and memory addresses for that process’s counter instance.
`emit!` updates the readings without resolving counter names.
Neither call installs the catalog entries.

For `PerfSpy`, separately configure your deployment’s EDS collection service
to collect the category and instances.
This crate exposes local counters; it does not send readings to a server.

## Behavior

Emission updates atomic `u64` storage without string lookup, allocation,
redaction, or Windows API calls. Windows collection snapshots these values
into registered reference cells. Events emitted through a sink follow that
sink’s timestamp, context, enrichment, and sampling rules; the backend does
not extract enrichment.

Values reset when the processor restarts. Snapshots are per-metric, not
transactional across an event. Brief collection contention keeps the previous
snapshot or returns a retryable busy response. Invalid callback state freezes
snapshots until restart and surfaces through `flush`; emission still records.

The conversion to a standard `Arc` above matches the `Sink::new` API boundary.
Native state is local to one executable or library image; never unload an
image with a live provider.

See the Microsoft [provider guide][__link1]
and [resource installation reference][__link2].

## Maintainer guides

See the [backend design and publication limits][__link3]
and [implementation guide][__link4].


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/observed_perf_counters">source code</a>.
</sub>

 [__link0]: https://github.com/microsoft/oxidizer/blob/main/crates/observed_perf_counters/docs/design.md#publication-safety-boundary
 [__link1]: https://learn.microsoft.com/windows/win32/perfctrs/providing-counter-data-using-version-2-0
 [__link2]: https://learn.microsoft.com/windows/win32/perfctrs/ctrpp
 [__link3]: https://github.com/microsoft/oxidizer/blob/main/crates/observed_perf_counters/docs/design.md
 [__link4]: https://github.com/microsoft/oxidizer/blob/main/crates/observed_perf_counters/docs/implementation.md
