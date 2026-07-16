<div align="center">
 <img src="./logo.png" alt="Observed Logo" width="96">

# Observed

[![crate.io](https://img.shields.io/crates/v/observed.svg)](https://crates.io/crates/observed)
[![docs.rs](https://docs.rs/observed/badge.svg)](https://docs.rs/observed)
[![MSRV](https://img.shields.io/crates/msrv/observed)](https://crates.io/crates/observed)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Structured telemetry events with enrichment, redaction, and per-field routing.

The `observed` crate provides a unified telemetry API that:

* Emits **structured, typed events** via `#[derive(Event)]` and the [`emit!`][__link0] macro
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
use observed::{Event, Sink, emit};

const DC: DataClass = DataClass::new("example", "public");

#[derive(Event)]
#[event(name = "my.event")]
#[log(severity = info, message = "Processing {my.event.field}")]
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

Enrichment is not automatically propagated to other threads or async tasks. It has to be
explicitly transferred via [`Sink::transfer_context`][__link5] and
[`Transfer::apply`][__link6].

How it works:

* [`Sink::transfer_context`][__link7] captures the current enrichment state into a plain data struct
  ([`Transfer`][__link8]).
* [`Transfer::apply`][__link9] restores the captured state on the
  target thread or in the spawned future’s poll. The returned guard restores the previous
  state on drop.

```rust
let transfer = sink.transfer_context();

let sink = sink.clone();
let handle = std::thread::spawn(move || {
    // Restore the captured enrichment on this thread for the guard's lifetime.
    let _guard = transfer.apply();
    emit!(sink, MyEvent); // sees parent enrichment
});
handle.join().unwrap();
```

### Resolution at emission time

When `emit!` fires, the sink walks its thread-local enrichment chain and
collects all visible entries and passes them to processors along with the event.


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/observed">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQbLiTyV0MU86EbZU15e0PmecoboQ9jo59bnAEbyDXw04U13GlhYvRhcoQb7KFLaHlptPMbfTfLsQWQKuwbWp2syCa7QWcbgu2wNwlFoH1hZIKCbGRhdGFfcHJpdmFjeWYwLjEyLjOCaG9ic2VydmVkZjAuMjMuMA
 [__link0]: `emit!`
 [__link1]: https://docs.rs/data_privacy/0.12.3/data_privacy/?search=RedactionEngine
 [__link2]: https://docs.rs/observed/0.23.0/observed/?search=processing::EventProcessor
 [__link3]: https://docs.rs/observed/0.23.0/observed/?search=enrichment::EnrichFutureExt::enrich
 [__link4]: https://docs.rs/observed/0.23.0/observed/?search=enrichment::EnrichFnExt::enrich
 [__link5]: https://docs.rs/observed/0.23.0/observed/?search=Sink::transfer_context
 [__link6]: https://docs.rs/observed/0.23.0/observed/?search=context::Transfer::apply
 [__link7]: https://docs.rs/observed/0.23.0/observed/?search=Sink::transfer_context
 [__link8]: https://docs.rs/observed/0.23.0/observed/?search=context::Transfer
 [__link9]: https://docs.rs/observed/0.23.0/observed/?search=context::Transfer::apply
