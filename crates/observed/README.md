<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Observed Logo" width="96">

# Observed

[![crate.io](https://img.shields.io/crates/v/observed.svg)](https://crates.io/crates/observed)
[![docs.rs](https://docs.rs/observed/badge.svg)](https://docs.rs/observed)
[![MSRV](https://img.shields.io/crates/msrv/observed)](https://crates.io/crates/observed)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Structured telemetry events with enrichment, redaction, and per-field routing.

The `observed` crate provides a unified telemetry API that:

* Emits **structured, typed events** via `#[event(...)]` and the [`emit!`][__link0] macro
* Supports **enrichment** - scoped, stackable, context-propagated entries
  attached to all events in scope (via RAII guards and `#[derive(Enrichment)]` structs)
* Enforces **redaction** - data-classification metadata on every field, redaction
  applied through a [`RedactionEngine`][__link1]
* Provides **per-field routing** - one event struct can produce logs and metrics with
  independent field subsets per signal
* Integrates with **OpenTelemetry** through pluggable [`EventProcessor`][__link2] implementations

## Quick Start

```rust
use data_privacy::{DataClass, Sensitive};
use observed::{Sink, emit, event};

const DC: DataClass = DataClass::new("example", "public");

#[event("my.event")]
#[info("Processing {my.event.field}")]
struct MyEvent {
    #[dimension(log = "my.event.field")]
    field: Sensitive<&'static str>,
}

fn do_something(sink: &Sink) {
    emit!(
        sink,
        MyEvent {
            field: Sensitive::new("val", DC)
        }
    );
    // do something
}
```

## Enrichment

Enrichment attaches key-value context to **every event** emitted within a scope.
Typical use cases include request IDs, user identifiers, or operation names that
should appear on all telemetry without being passed explicitly to each event.

### Scoped enrichment

Use the [`EnrichFutureExt::enrich`][__link3] or
[`EnrichFnExt::enrich`][__link4] extension
methods to attach entries to a future or closure. The entries are pushed onto
the thread-local slot on every poll (or call) and popped when the poll
completes:

```rust
#[derive(Enrichment)]
struct RequestCtx {
    #[dimension(log = "request.id")]
    request_id: RequestId,
}

async fn fetch(request_id: RequestId, sink: &Sink) {
    async {
        emit!(sink, MyEvent::new("test")); // sees request.id
    }
    .enrich(sink, RequestCtx { request_id })
    .await;
}
```

### Transferring enrichment across threads and tasks

Enrichment lives in a thread-local slot, so it is **not** automatically
propagated to other threads or async tasks.

**Most code should not do this by hand.** The runtime integrations
(`observed_rt`, which layers over `anyspawn`, and `oxidizer_rt`) propagate
enrichment to every spawned task for you: enrich at the spawn site, spawn
through the runtime, and the context follows. The rest of this section is
the plumbing underneath, and is aimed at people writing such an integration -
a spawner, a `tower` layer, or similar middleware.

Integrators transfer it explicitly:

* [`Sink::transfer_context`][__link5] snapshots the current thread’s enrichment into a
  plain, sendable [`Transfer`][__link6] value.
* [`EnrichFutureExt::attach`][__link7] wraps a
  future so the captured enrichment is restored **on every poll** and removed
  again before the future yields.

Applying a transfer mutates the enrichment of the current thread for the
lifetime of the returned guard, so any emission made through the original sink
on that thread also sees the transferred entries.

To add entries of your own, put them **in the transfer** with
[`Transfer::with_enrichment`][__link8] or
[`Transfer::with_enrichment_for`][__link9].
That is what an integration wants: it is independent of wrapper order, so it
keeps working once the future is boxed or wrapped again further out - both of
which happen in real integrations.

Wrapping [`enrich`][__link10] around
`attach` also works for a single `attach` on a plain, non-boxed future, since
[`Transferred::enrich`][__link11] re-orders the two.
That is a convenience for hand-written code, not a general guarantee - see its
docs for the shapes it does not cover.

```rust
// Capture the current thread's enrichment as a plain, sendable value...
let transfer = sink.transfer_context();

// ...and attach it to the future that will run on another task/thread.
// `attach` restores the enrichment on every poll and drops it before the
// future yields, so unrelated tasks on the same worker thread never see it.
let sink = sink.clone();
let task = async move {
    emit!(sink, MyEvent); // sees the transferred enrichment
}
.attach(transfer);
// Hand `task` to your executor, e.g. `tokio::spawn(task)`.
let _ = task;
```

For synchronous work you can instead apply the low-level guard directly via
[`Transfer::apply_current_thread`][__link12].
**Never hold that guard across an `.await`**: because it mutates a thread-local,
the enrichment would stay active while the task is suspended and leak into
unrelated tasks that the runtime schedules on the same thread. Use
[`attach`][__link13] for async code so the
guard is scoped to a single poll.

### Resolution at emission time

When `emit!` fires, the sink walks its thread-local enrichment chain and
collects all visible entries and passes them to processors along with the event.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/observed">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQb68x7sQbVDdYbwXFbW_bB1wAb0GrV6MyUj6kbkecovaJAvyFhZIKCbGRhdGFfcHJpdmFjeWYwLjEyLjSCaG9ic2VydmVkZjAuMjQuMA
 [__link0]: `emit!`
 [__link1]: https://docs.rs/data_privacy/0.12.4/data_privacy/?search=RedactionEngine
 [__link10]: https://docs.rs/observed/0.24.0/observed/?search=enrichment::EnrichFutureExt::enrich
 [__link11]: https://docs.rs/observed/0.24.0/observed/?search=context::Transferred::enrich
 [__link12]: https://docs.rs/observed/0.24.0/observed/?search=context::Transfer::apply_current_thread
 [__link13]: https://docs.rs/observed/0.24.0/observed/?search=enrichment::EnrichFutureExt::attach
 [__link2]: https://docs.rs/observed/0.24.0/observed/?search=processing::EventProcessor
 [__link3]: https://docs.rs/observed/0.24.0/observed/?search=enrichment::EnrichFutureExt::enrich
 [__link4]: https://docs.rs/observed/0.24.0/observed/?search=enrichment::EnrichFnExt::enrich
 [__link5]: https://docs.rs/observed/0.24.0/observed/?search=Sink::transfer_context
 [__link6]: https://docs.rs/observed/0.24.0/observed/?search=context::Transfer
 [__link7]: https://docs.rs/observed/0.24.0/observed/?search=enrichment::EnrichFutureExt::attach
 [__link8]: https://docs.rs/observed/0.24.0/observed/?search=context::Transfer::with_enrichment
 [__link9]: https://docs.rs/observed/0.24.0/observed/?search=context::Transfer::with_enrichment_for
