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

An externally authored Windows V2 `.man` file defines the provider, counter
sets, counter IDs, names, and native types. Deployment installs that definition
into Windows’ catalog; the application reads the same file at startup and
binds Rust metric names to its declared counters. Multiple processes can
publish the same definitions through separate named instances.

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
    .manifest_file("MyServiceCounters.man")?
    .category("MyService")
    .instance(format!("worker-{}", id()))
    .bind_event::<RequestCompleted>();

// Application startup uses the identities from the supplied definition.
let processor = Arc::into_std_arc(Arc::new(builder.build()?));
let sink = Sink::new("app", vec![processor], SimpleClock::new_system());
emit!(sink, RequestCompleted { bytes_sent: 4096 });
```

`category` selects a counter set by its manifest `name`; it does not create a
category or derive a GUID. `bind_event` maps every instrument on a typed event
by exact instrument-name match, once during startup. Each name must identify
one counter in the selected set, and two instruments cannot bind the same
counter. Counter IDs and GUIDs are never generated from Rust metadata.

Unbound counters in the selected set remain registered at zero. Application
units and descriptions do not rewrite Windows deployment metadata.

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

**Use the deployment-owned definition.** Windows cannot infer counter
definitions from Rust events. Supply the `.man` maintained by your deployment,
with instrument names matching its counters. The
[example definition][__link1]
shows the supported format. This crate reads it; it does not export XML.

Compile that same supplied manifest’s names and descriptions into the resource
binary named by its `applicationIdentity`. For the example definition:

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

**Prepare each process at startup.** `manifest_file` reads UTF-8 XML once.
`build()` validates the selected set and all event mappings, then registers
the supplied GUIDs, IDs, and backing storage on 64-bit Windows. Unknown
categories, missing or conflicting mappings, unsupported definitions, and
malformed files are errors, not new or no-op counters. `emit!` uses numeric
targets with no file access or name matching. Neither call installs metadata.

The supported manifest profile is user-mode, multiple-instance counter sets
containing between one and 256 inline-named `perf_counter_large_rawcount`
reference counters, with a custom callback, standard detail and zero/default
scale. Parsing checks this backend profile, not the complete SDK schema;
deployment should also validate with `ctrpp`. Other native
types, resource-ID-only names, and unsupported attributes are rejected.
Files must be at most one MiB; document type declarations are rejected.
This imports V2 definitions, not classic .NET custom counter categories.

For `PerfSpy`, separately configure your deployment’s EDS collection service
to collect the category and instances.
This crate exposes local counters; it does not send readings to a server.

## Behavior

Emission uses the lazy event-field visitor and prepared numeric indices to
update atomic `u64` storage without string lookup, allocation, redaction, or
Windows API calls. Only bound numeric getters run; other fields are enumerated
without evaluating their values. Windows collection snapshots these values
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

See the Microsoft [provider guide][__link2]
and [resource installation reference][__link3].

## Maintainer guides

See the [backend design and publication limits][__link4]
and [implementation guide][__link5].


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/observed_perf_counters">source code</a>.
</sub>

 [__link0]: https://github.com/microsoft/oxidizer/blob/main/crates/observed_perf_counters/docs/design.md#publication-safety-boundary
 [__link1]: https://github.com/microsoft/oxidizer/blob/main/crates/observed_perf_counters/examples/MyServiceCounters.man
 [__link2]: https://learn.microsoft.com/windows/win32/perfctrs/providing-counter-data-using-version-2-0
 [__link3]: https://learn.microsoft.com/windows/win32/perfctrs/ctrpp
 [__link4]: https://github.com/microsoft/oxidizer/blob/main/crates/observed_perf_counters/docs/design.md
 [__link5]: https://github.com/microsoft/oxidizer/blob/main/crates/observed_perf_counters/docs/implementation.md
