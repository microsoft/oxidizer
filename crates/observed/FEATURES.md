# observed - Key Features

## Event Definition

- **Typed events** - `#[derive(Event)]` generates compile-time validated telemetry structs
- **Per-signal attributes** - struct-level `#[event(name = "...")]` declares the canonical event name; `#[log(severity = ..., message = "...")]` and/or a `#[metric(kind = ...)]` instrument attribute declare which signals the event produces (at least one is required); log/metric names default to the event name; `#[disabled]` marks the event as opt-in only
- **Event-level metric declaration** - a `#[metric(kind = <Kind>, ...)]` attribute on the struct declares an instrument; `kind = counter` may be fieldless and records `1` per emission, while `updown_counter`, `gauge`, and `histogram` require `field = <ident>` naming the struct field supplying the metric value
- **Per-field routing** - default is log-only; `#[dimension(metric)]` (own-name key) or `#[dimension(metric = "...")]` (explicit key) marks a field as a metric dimension; an instrument attribute's `field = <ident>` names the field whose value becomes the metric value; `#[dimension(log = exclude)]` opts a field out of log routing; `#[dimension(log = "...")]` renames the log key; a bare `#[dimension]` is the explicit log-only default
- **Optional-field placeholders** - `Option` fields emit `"n/a"` when `None` by default; `#[if_none("...")]` sets a custom placeholder and `#[if_none(drop)]` omits the field entirely when `None`
- **Value-type enforcement** - `#[metric(kind = counter, field = ...)]` requires the value field to be an unsigned integer, and `#[metric(kind = updown_counter, field = ...)]` requires a signed integer; a mismatch is a compile-time error
- **Field redaction** - `#[unredacted]` bypasses redaction; `#[data_class(<expr>)]` wraps the value in `Sensitive` for classification
- **Lazy event construction** - `emit!` only constructs the event if at least one processor is interested
- **Source location** - automatic `code.file.path` and `code.line.number` on every record

## Context & Enrichment

- **Explicit sink passing** - `emit!(sink, event)` takes an `&Sink` as first argument; no ambient/global state
- **Scoped enrichment** - `.enrich(&sink, entries)` on closures/futures attaches key-value context to all events in a scope
- **Per-sink thread-local storage** - each `Sink` owns a per-thread enrichment slot backed by `thread_local` crate;
  enrichments are pushed onto a per-thread linked list with RAII guards
- **Typed enrichment structs** - `#[derive(Enrichment)]` converts structs into enrichment entries with the same field-level
  attributes as `Event` (`#[dimension(...)]`, `#[if_none(...)]`, `#[data_class]`, `#[unredacted]`);
  enrichment can opt into metric dimensions via `#[dimension(metric = "...")]` but cannot define a metric instrument
- **Per-sink enrichment** - `.enrich_for(&sink, target, entries)` targets context to a specific sink
- **Enrichment isolation** - library sinks can opt out of global enrichments via `Sink::new_isolated(ID, processors, clock)`
- **Cross-thread context transfer** - `sink.transfer_context()` captures enrichment state for cross-thread propagation

## Sink Lifecycle

- **Direct constructors** - `Sink::new(ID, processors, clock)` and `Sink::new_isolated(ID, processors, clock)` for foundation-level construction; `clock` is anything `AsRef<tick::SimpleClock>` (both `tick::SimpleClock` and `tick::Clock`) and stamps every event's timestamp
- **O(1) clone** - `Sink` cloning is cheap
- **Composite sinks** - `Sink::composite([a, b, …])` returns a sink that dispatches every event through each child in turn
- **Noop sink** - `Sink::noop()` creates a sink with no processors for testing

## Emission & Routing

- **Processor-based dispatch** - `emit!(sink, ...)` sends to the sink's processors via `EventProcessor::process`
- **Lazy event views** - each processor receives an `EventView` and pulls only the fields it needs; skipped fields never invoke their redaction closure (zero cost for rejected fields)
- **Allocation-free static keys** - all field, enrichment, and interop keys are `&'static str`, so `Key`/`FieldDescriptor`/`LogFieldEntry`/`MetricFieldEntry` are `Copy` and snapshotting consumers (e.g. snapshotting/replay processors) retain keys with zero allocation. The `tracing` bridge forwards `tracing`'s `&'static` field/target/file names directly instead of cloning them.
- **Interest-based lazy construction** - processors implement `is_interested(&EventDescription)` (default `true`); if all return `false`, the event closure is never called
- **Per-processor filtering** - processors filter inside `process()` using `EventView::description()` (signal type, severity, etc.)
- **OpenTelemetry integration** - built on `opentelemetry_sdk` log and metric providers (value-layer only; no OTel Context dependency)
- **Foreign event interoperability** - the `interop` module exposes the type-erased `DynEvent` trait and `emit_dyn_event` entry point for bridging events from other telemetry crates (e.g. `tracing`, `log`) that cannot implement the typed `Event` trait. These adapted events flow through the sink's normal pipeline (enrichment, interest checks, processors). `interop` types are **not** re-exported at the crate root; reach them via `observed::interop::{DynEvent, emit_dyn_event}`.

## Privacy

- **Three-path field classification** - every field follows one of three redaction paths:
  1. **Default** - the type must implement `RedactedDisplay` (e.g. via `#[classified(...)]`). Compilation fails if it doesn't.
  2. **`data_class = <expr>`** - wraps the value in `Sensitive::new(value, expr)` before redaction, for types without built-in classification.
  3. **`unredacted`** - bypasses redaction entirely; the type must implement `Into<Value>`.
- **Redaction** - `Sensitive<T>` + `RedactionEngine` enforce privacy-by-construction
- **Per-processor redaction** - each processor owns its own `RedactionEngine` privately, passing it to getter closures during `visit_fields`/`visit_enrichments`

```text
emit!(sink, MyEvent { a: expensive() })
-> sink.emit::<MyEvent, _>(|| MyEvent { a: expensive() }, source_location)
  │
  ├── noop check: if sink.is_noop() -> early exit (no processors)
  │
  ├── interest check: any processor interested in MyEvent::DESCRIPTION?
  │     -> if none interested, early exit (event closure NOT called)
  │
  ├── construct event: let event = closure()
  │
  ├── resolve enrichments: walk per-sink TLS linked list
  │
  ├── build EventView(event, enrichments) - zero-cost, just a pair of references
  │
  └── for each processor (broadcast):
        └── processor.process(&event_view)
              │   (processor filters internally via event_view.description())
              │
              ├─ LogEventProcessor (destination crate)
              │    event_view.visit_fields(|desc, getter| {
              │      if desc.log().is_some() { getter(&engine) }
              │    })
              │    -> SdkLoggerProvider -> Logger -> LogRecord
              │      -> LogProcessor (installed by LogDestination)
              │        ├─ SimpleLogProcessor -> StdoutLogExporter -> stdout
              │        ├─ BatchLogProcessor  -> WriteExporter   -> file (background thread)
              │        └─ (ETW, memory, …)
              │
              └─ MetricEventProcessor (destination crate)
                   event_view.visit_fields(|desc, getter| {
                     if let Some(metric) = desc.metric() {
                       if metric.instrument_description().is_some() {
                         // field metric value: record getter(&engine).to_number()
                       } else {
                         // metric dimension: getter(&engine) -> KeyValue
                       }
                     }
                   })
                   // event-level metric from description.metric() records 1.0
                   -> SdkMeterProvider -> Meter -> instruments
                     -> MetricReader (installed by MetricDestination)
                       ├─ PeriodicExporter -> StdoutMetricExporter -> stdout
                       └─ (ETW, memory, …)
```
