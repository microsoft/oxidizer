// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Publish `observed` integer metrics as Windows performance counters.
//!
//! Windows keeps a machine-wide catalog mapping counter names to IDs and numeric
//! formats; applications hold the live readings in memory. At startup, `build()`
//! registers the matching IDs and memory addresses, letting monitoring tools find
//! a counter by name and ask Windows for its reading. Multiple application
//! processes can publish the same counters, each through a named instance.
//!
//! # Usage
//!
//! ```no_run
//! use observed::{Sink, emit, event};
//! use observed_perf_counters::PerfCounterProcessor;
//! use performables::arc::Arc;
//!
//! #[event("request.completed")]
//! #[counter(bytes_sent, name = "Bytes Sent", unit = "By")]
//! struct RequestCompleted {
//!     #[unredacted]
//!     bytes_sent: u32,
//! }
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let builder = PerfCounterProcessor::builder()
//!     .category("MyService")
//!     .instance(format!("worker-{}", std::process::id()))
//!     .bind_event::<RequestCompleted>();
//!
//! // Deployment tooling: save these definitions as MyServiceCounters.man.
//! let xml = builder.manifest("MyServiceCounters.dll")?;
//!
//! // Application startup: create this process's counter instance.
//! let processor = Arc::into_std_arc(Arc::new(builder.build()?));
//! let sink = Sink::new("app", vec![processor], tick::SimpleClock::new_system());
//! emit!(sink, RequestCompleted { bytes_sent: 4096 });
//! # Ok(())
//! # }
//! ```
//!
//! `bind_event` registers all instruments on the event using their declared
//! names, units, and descriptions. Names must be unique within the category.
//!
//! | Instrument | Value | Meaning |
//! |------------|-------|---------|
//! | `counter` | Unsigned integer, or no field | Add to the total; no field adds one. |
//! | `updown_counter` | Signed integer | Adjust the total, which cannot fall below zero. |
//! | `gauge` | Unsigned integer | Replace the previous reading. |
//!
//! Histograms, floats, and signed gauges are unsupported. Overflow or underflow
//! leaves that metric unchanged, logs the first error through `tracing`, and
//! returns it from an explicit `Sink::flush()`. Other valid updates continue.
//! `flush` does not trigger Windows collection. Use an unsampled sink for deltas.
//!
//! # Deployment
//!
//! **Register the definitions with Windows.** Windows cannot infer counters from
//! your Rust events. `builder.manifest("MyServiceCounters.dll")` generates XML
//! mapping their names to stable IDs, numeric formats, and descriptions.
//! Save it as `MyServiceCounters.man`; the Windows SDK compiles its names and
//! descriptions into a resource DLL:
//!
//! Run these commands in a Windows SDK Developer Command Prompt:
//!
//! ```text
//! ctrpp -o MyServiceCounters.h -rc MyServiceCounters.rc MyServiceCounters.man
//! rc /fo MyServiceCounters.res MyServiceCounters.rc
//! link /dll /noentry /machine:x64 /out:MyServiceCounters.dll /implib:MyServiceCounters.lib MyServiceCounters.res
//! ```
//!
//! Ship the `.man` and DLL with your application. An elevated installer runs
//! `lodctr /m:"C:\Program Files\MyService\MyServiceCounters.man"` to add these
//! definitions to Windows' catalog. Register them once on each computer, not
//! for each running process; repeat when your declared metrics change.
//! The DLL contains descriptive text, not Rust code or live counter values.
//! For ARM64, replace `/machine:x64` with `/machine:arm64`.
//!
//! **Publish from each process.** On 64-bit Windows, `build()` registers the IDs,
//! numeric formats, and memory addresses for that process's counter instance.
//! `emit!` updates the readings without resolving counter names.
//! Neither call installs the catalog entries.
//!
//! For `PerfSpy`, separately configure EDS to collect the category and instances.
//! This crate exposes local counters; it does not send readings to a server.
//!
//! # Behavior
//!
//! Emission updates atomic `u64` storage without string lookup, allocation,
//! redaction, or Windows API calls. Windows collection snapshots these values
//! into registered reference cells. Existing `observed` sink timestamp and
//! enrichment bookkeeping is unchanged; the backend does not extract enrichment.
//!
//! Values reset when the processor restarts. Snapshots are per-metric, not
//! transactional across an event. Collection callback failures freeze published
//! snapshots until restart and surface through `flush`; emission still records.
//!
//! Shared ownership and locks use `performables`; standard atomics remain for
//! numeric updates. The conversion to a standard pointer above is only the
//! existing `Sink::new` API boundary. Native state is local to one executable or
//! library image; never unload an image with a live provider.
//!
//! See the Microsoft [provider guide](https://learn.microsoft.com/windows/win32/perfctrs/providing-counter-data-using-version-2-0)
//! and [resource installation reference](https://learn.microsoft.com/windows/win32/perfctrs/ctrpp).

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

mod error;
#[cfg(all(windows, target_pointer_width = "64"))]
mod native;
mod processor;
mod schema;

pub use error::Error;
pub use processor::{PerfCounterProcessor, PerfCounterProcessorBuilder};

#[cfg(test)]
testing_aids::init_tracing!();
