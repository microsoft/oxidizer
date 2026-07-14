# observed - Telemetry Framework Design

`observed` is a collection of crates that provide a telemetry frontend solution.

## Motivation

Rust services and libraries need structured telemetry that is:

- **Decoupled** - emit sites do not depend on specific exporters or the OpenTelemetry SDK.
- **Privacy-safe by construction** - classified data cannot reach exporters without passing through a redaction engine.
- **Easy to use** - telemetry should not repel people from using it.
- **Unified telemetry** - a single event definition can produce any combination of logs, metrics, and traces without duplication.
- **Propagation of enrichment** - attributes propagate semi-automatically through sync and async code.
- **Richer enrichment** - OTel attributes are limited to key-value pairs, and they lose additional information about an event and its dimensions, such as taxonomy.
- **Library-friendly** - library crates can emit telemetry using their telemetry pipeline without requiring callers to perform any setup.

## Requirements

1. **Typed, compile-time validated events/enrichment.** Every telemetry event is a Rust struct.
   Field names and types are checked at compile time. No string-based, unvalidated telemetry.
   NOTE: We decided to limit v1 to typed events/enrichment only, without allowing arbitrary
   key-value pairs. If there is demand for it, we can add it later.

1. **Single call for all signals.** One `emit!()` call can emit a structured log record,
   record a metric observation, and participate in a trace span
   from the same event struct. No separate calls per signal.

1. **Privacy-by-construction via redaction.** All non-primitive attribute values - whether
   defined on the event struct or added by enrichment - must pass through a **redaction engine**
   before reaching any exporter. The type system makes it impossible to accidentally emit
   classified data. See Appendix #1.

1. **Scoped, automatic enrichment.** Attributes attach to all events within a scope and propagate
   through nested calls - including across `.await` points and thread migrations.

1. **Per-field/per-enrichment-field routing control.** Each field can be annotated to indicate
   inclusion in or exclusion from each signal (logs, metrics, traces).
   These annotations are advisory - processors may opt-in to respect them but are not required to.
   Dimensions excluded from all active signals should have minimal impact on performance.

1. **Runtime event schema changes.** The defaults for which signals an event produces are determined **at compile time**, but can be overridden at runtime.

1. **Library and application telemetry isolation.** Libraries define the defaults for applications
   using that library, and on top of that, they can set up their own pipelines which diverge from
   those defaults if they want more or less detail.

1. **OpenTelemetry-native export.** Events are processed through standard `SdkLoggerProvider` and `SdkMeterProvider`, enabling any OTel-compatible exporter.

1. **Zero-cost when inactive.** Emitting must have zero overhead in the following scenarios:

- No exporters are configured.
- The event is disabled.
- Low severity: severity pre-filtering skips event construction entirely - no field extraction, no allocation, no redaction call.
- Signal-level: if an emission results only in a metric, you should not pay the cost of logging - and vice versa.

1. **Enrichment crates.** A dedicated crate (e.g., `auth_attributes`, `http_context`) can define a public enrichment type reusable by other crates.

1. **Flat composition.** Multiple attribute sources (HTTP context, auth claims, tenant info,
   infrastructure metadata, tracing context) must compose without **nesting** closures.
   Five attribute providers should not produce five levels of indentation.

1. **Events and enrichment are runtime agnostic.** Everything should work independently of any
   async runtime. There might be some additional requirements for how to propagate context
   between tasks/threads.
   NOTE: That also includes propagation of native OTel `Context`/spans in case a library uses the `opentelemetry` crate directly.

1. **Zero-cost bypass for fields that don't require redaction.** Fields annotated with `unredacted` skip the redaction engine entirely (no `RedactedDisplay` call, no allocation).
   Also applicable to fileds that have `DataClass` not requiring any redaction.

1. **Rust stable toolchain.** The crate targets stable Rust (no nightly-only features). Proc
   macros must work with the minimum supported Rust version (MSRV) defined by the workspace.
   In general, MSRV is updated every other Rust release.

1. **Telemetry must never block or crash the application.** All telemetry paths are best-effort. Telemetry failures must not propagate as application errors.

1. **Testability.** The crate should provide the means for testing telemetry. It should be
   possible to run multiple tests in parallel without one test affecting another.

1. **Event emission is "sync".** It cannot execute any async code.

1. **Non-blocking, low-contention emit.** `emit!` should not perform any blocking I/O and
   should strive to result in thread-isolated work with little or no contention with other
   threads in the process.
   NOTE: We cannot guarantee that users' interceptors/callbacks will not do any of this. The only thing that is not allowed is making async calls from `emit!`.

1. **No globals/statics.** Global state leads to unexpected behavior, complicates testing,
   and breaks when multiple versions of the crate coexist in the dependency tree.

## *Nice-to-have* requirements

1. **Avoid large dependencies.** The `observed` crate should be relatively lightweight to avoid
   blowing up the dependency tree of its users. For example, it must not link `opentelemetry_sdk`,
   but it can depend on the `opentelemetry` crate.

1. **Automatic enrichment scopes.** Automatically attach enrichment when crossing crate borders
   without requiring consumers to manually call `enrich()` at every entry point.

## Non-Requirements

1. **Metrics aggregation or alerting.** `observed` records metric observations (histogram values,
   gauge readings). Aggregation, percentile computation, and alerting are the responsibility of
   the metrics backend (e.g. Prometheus).

1. **Automatic instrumentation of HTTP/gRPC frameworks.** `observed` provides instruments for manual
   enrichment of the current scope. It does not auto-instrument middleware stacks. Integration
   layers (e.g., layer for Tower) are provided separately.

## Assumptions and Constraints

1. **Dependency on `data_privacy` crate.** The `RedactionEngine` and instrumentation for labeling
   custom types with taxonomy are provided by the `data_privacy` crate. `observed` depends on this
   crate for redaction; it does not implement its own redaction logic. Libraries using the `observed`
   crate must adopt `data_privacy` labeling for their value types. The `data_privacy` crate is a
   first-party crate and is part of the Oxidizer project.

1. **Async propagation is manual.** Async code must explicitly propagate enrichment context
   and library pipelines across `.await` points. The `observed` crate may or may not provide helpers
   for doing that.

1. **ETW as a primary export target.** While `observed` supports any OTel-compatible
   exporter, the ETW exporters (`opentelemetry-etw-logs`, `opentelemetry-etw-metrics`)
   are a key deployment target. All custom exporters should function without taking a dependency
   on the observed crate.

1. The `observed` crates don't do static enrichment; it should be done via an OTel `Resource` object.

## Trustworthiness

### Security

1. **No secret material in telemetry.** `observed` does not handle authentication tokens, API keys,
   or cryptographic material. Secrets must never appear as attributes. The `data_class` annotation
   system helps enforce this by requiring explicit classification of all value fields, but it is
   the responsibility of the event author to classify correctly.

1. **No network I/O in the core crate.** The `observed` crate itself performs no network operations.
   All network-facing behavior (HTTP export, OTLP, ETW) lives in separate destination crates or
   downstream exporters. This limits the attack surface of the core path.

1. **No user-controlled format strings.** Event names, field names, and body templates are
   static `&'static str` values generated at compile time by proc macros. There is no runtime
   string interpolation that could enable injection attacks.

### Privacy

1. **Redaction-by-construction.** The core privacy guarantee: attribute values cannot reach an exporter without an explicit classification decision.
   Every field in `#[derive(Event)]` and `#[derive(Enrichment)]` follows one of three paths:
   - **Default** - the type must implement `RedactedDisplay` (e.g. via `#[classified(...)]`). If it doesn't, compilation fails.
   - **`data_class = <expr>`** - wraps the value in `Sensitive::new(value, expr)` before redaction, for types that don't carry their own classification.
   - **`unredacted`** - bypasses redaction entirely; the type must implement `Into<Value>`. Used for primitives and other inherently non-sensitive values.

   `data_class` and `unredacted` are mutually exclusive (compile error if both specified).

1. **Data classification annotations.** Attributes carrying personal or sensitive data must be labeled with an appropriate `DataClass`.
   This annotation feeds into the redaction engine's policy decisions.

1. **No telemetry of telemetry.** `observed` does not log its own internal operations (dropped events, lock contention, channel back-pressure) through itself.
   Internal diagnostics, if added, must use a separate mechanism to avoid circular dependencies and accidental data leakage.

## Core Concepts

### Sink

A **Sink** is a composable event dispatcher identified by a `SinkId`. It is the unit of telemetry configuration - each sink independently owns:

- One or more **pipeline** instances (each with its own OTel providers, `RedactionEngine`, and `ProcessingInstructions`).
- An optional **severity prefilter**.
- An **enrichment isolation** flag.

A new instance of a sink can be created from an existing one. It will inherit the setup of the previous sink and add new pipelines/instructions on top.

Sinks are activated in the current scope via RAII guards. `emit!()` always accepts a sink as the first argument and fans out to all active sinks,
each processing the event independently through its pipelines. Processing instructions are generated at compile time by `#[derive(Event)]`,
cached per event type on first emission, and can be overridden at runtime. See [Sinks and Keys](#sinks-and-keys) for the full technical details.

### Signal Routing: How Events Become Logs, Metrics, and Traces

An event can produce any combination of signals defined by its schema.

#### Logs

1. **By default, every event always produces a log record.** By default, all attributes are included in logs, but there is a way to opt-out.

#### Metrics

1. **A metric is produced only when an event or an event field is marked as a metric type.** If there is no such attribute in an event, the event produces no metrics.

1. **Metric dimensions are strictly opt-in.** An event field or enrichment field becomes a metric
   dimension only when it is explicitly marked with `#[dimension(metric = "...")]`. Unmarked fields are
   never added as dimensions, even on metric-only events.

1. Enrichment fields opt into metric dimensions the same way event fields do - via `#[dimension(metric = "...")]`.
   The `#[derive(Enrichment)]` macro does not support instrument attributes (enrichment cannot *be* a
   metric value), only dimension opt-in.

1. An instance of a metric instrument is created automatically when a metric is emitted for the first time.
   Instruments are stored as thread-local instances (we need to check if OTel is already doing this).

1. Every "pipeline" has its own instance of a metric instrument that publishes results to its own target.

In summary, the routing is determined by annotations on the event struct:

| Scenario | Log? | Metric? | Trace? |
| --- | --- | --- | --- |
| No instrument attribute | Yes | No | If span active |
| One instrument attribute | Yes | Yes (one instrument) | If span active |
| Multiple instrument attributes | Yes | Yes (per instrument) | If span active |

## Technical details

### Event properties

Each event uses per-signal attributes at the struct level; both `#[log(...)]` and the instrument attributes are optional.

| Annotation | Effect |
| --- | --- |
| `#[event(name = "...")]` | **Required.** Declares the canonical event name used for routing and identification. |
| `#[log(severity = <ident>, message = "...")]` | Declares the event as a log. `severity` is one of `trace`, `debug`, `info`, `warn`, `error`, `fatal`. `name` defaults to the event name; `message` is optional. |
| `#[metric(kind = <Kind>[, name = "..."][, field = ...])]` | Declares an event-level metric instrument, where `<Kind>` is `counter`, `updown_counter`, `gauge`, or `histogram`. `name` defaults to the event name. `kind = counter` may be fieldless and records `1` per emission; the others require `field = <ident>` naming the struct field that supplies the metric value. |
| `#[disabled]` | The event is opt-in: by default no processor receives it; processors must explicitly opt in. |

### Dimension (field) properties

Field-level attributes control routing and redaction. By default, every field is a log attribute and is redacted; fields are metric dimensions only when explicitly marked.

| Annotation | Effect | Comment |
| --- | --- | --- |
| `#[dimension]` | Log under the field's own name; not a metric dimension (the explicit default) | |
| `#[dimension(log = "...")]` | Rename the log key | |
| `#[dimension(log = exclude)]` | Omit the field from log records | |
| `#[dimension(metric)]` | Register the field as a metric dimension keyed by the field's own name | |
| `#[dimension(metric = "...")]` | Include the field as a metric dimension under the given key | |
| `#[dimension(log = "...", metric = "...")]` | Route both signals with independent keys; either side may be omitted (but not both). `log = exclude` omits the field from logs, and a bare `metric` uses the field name | |
| `#[if_none("...")]` | For `Option` fields: emit the given placeholder when the value is `None` (default is `#[if_none("n/a")]`) | Only valid on `Option` fields |
| `#[if_none(drop)]` | For `Option` fields: omit the field entirely when the value is `None` | Only valid on `Option` fields |
| instrument `field = ...` | Names the field whose value IS the metric value for the instrument | The referenced field must not also be a metric dimension; `kind = counter` requires unsigned, `kind = updown_counter` signed |
| `#[data_class(<expr>)]` | Data-classification expression; wraps the value in `Sensitive::new(value, expr)` before redaction | Mutually exclusive with `#[unredacted]` |
| `#[unredacted]` | Bypass redaction; the type must implement `Into<Value>` | Mutually exclusive with `#[data_class(...)]` |

### Sinks and Keys

#### SinkId

A `SinkId` is a lightweight, `Copy` identifier for a sink. It wraps a `&'static str` label and can be defined as a `const` / `static` item.
Two keys are equal only if they represent the same instance.

#### Sink

A **Sink** is a composable event dispatcher identified by a `SinkId`. It is the unit of telemetry configuration - each sink independently controls:

- **Pipelines** - A sink holds ~~zero~~ one or more "pipeline" instances (there is no upper limit).
  When an event is emitted, every pipeline attached to the sink processes the event independently. This allows a single sink
  to send telemetry to multiple destinations simultaneously (e.g. stdout for development, ETW for production, an in-memory buffer for testing).

- **Severity prefilter** - An optional minimum severity threshold.
  Events below this level are dropped before any field extraction, enrichment resolution, or redaction occurs.

- **Enrichment isolation** - By default, a sink receives both global enrichments (from `.enrich()`) and targeted enrichments (from `.enrich_for()`).
  When isolation is enabled, the sink only sees targeted enrichments addressed to its id.
  This lets library authors keep their internal telemetry independent of the hosting application's enrichment context.

#### SinkPipeline

An `SinkPipeline` is a single emission target. It bundles OTel providers (`SdkLoggerProvider`, `SdkMeterProvider`),
a `RedactionEngine`, and its own set of processing instructions. Each pipeline maintains its own processing instructions independently;
the same event type can be routed differently by different pipelines within the same sink.

#### Scoped (lib) sinks

**Scoped sinks** - registered in the current scope via RAII guards. `emit!()` fans out to all such sinks available in the current scope.
Multiple sinks can be attached simultaneously - they compose additively. When the guard drops, the sink is removed and the previous state is restored.

#### How sinks, pipelines, and destinations relate

```text
Sink (identified by SinkId)
├─ severity prefilter
├─ enrichment isolation flag
├─ SinkPipeline #1
│  ├─ SinkData (SdkLoggerProvider, SdkMeterProvider, RedactionEngine)
│  └─ ProcessingInstructions per event name/type
│     ├─ LogProcessingInstructions  -> Logger from the provider
│     └─ MetricProcessingInstructions -> Meter + cached instrument from the provider
├─ SinkPipeline #2
│  └─ (same structure, different providers/instructions)
└─ ...
```

### Processing of emitted event

In v1, processing will be done on the same thread that calls `emit!`.

:::mermaid
flowchart TD
    emit["<b>Emit Site</b><br/>emit!(HttpRequest { method, url, status, duration_ms })"]
    filter["<b>Prefiltering</b><br/>Only by severity at the moment"]
    dimensions["<b>Collect dimensions</b><br/>Event + Enrichment from thread-local OTel Context"]

    emit --> filter --> dimensions

    dimensions --> loop

    subgraph loop ["For each Sink enabled for this scope"]
        redact["<b>Redaction</b><br/>Pass all sensitive dimensions through RedactionEngine defined by target"]
        redact --> etw["<b>ETW Provider</b><br/>Logs + Traces via ETW"]
        redact --> metrics["<b>Metrics Exporter</b><br/>Metric observations + dimensions"]
        redact --> file["<b>File Logger</b><br/>Structured log records to file"]
        redact --> stdout["<b>stdout</b><br/>Human-readable log output"]
    end
:::

## Appendix #1: supported data types

| Base type | OTel representation |
| --- | --- |
| `bool` | `BoolValue` |
| `i8` / `i16` / `i32` / `i64` | `IntValue` (i64) |
| `u8` / `u16` / `u32` | `IntValue` (i64) |
| `u64` | `IntValue` (i64, may truncate) |
| `f32` / `f64` | `DoubleValue` (f64) |
| `Vec<bool>` | `ArrayValue(BoolValue)` |
| `Vec<i64>` | `ArrayValue(IntValue)` |
| `Vec<f64>` | `ArrayValue(DoubleValue)` |
| `String` / `&str` | `StringValue` |
| `Vec<String>` | `ArrayValue(StringValue)` |
| `Vec<u8>` / `Bytes` | `BytesValue` |

## Appendix #2: Acronyms and Definitions

| Term | Definition |
| --- | --- |
| **ETW** | Event Tracing for Windows - a high-performance kernel-level tracing facility. |
| **Sink** | A named telemetry pipeline identified by an `SinkId`. Holds one or more `SinkPipeline` instances and configuration (severity filter, enrichment isolation). |
| **SinkId** | A `&'static`-lifetime identifier for a sink. Uses pointer identity for O(1) equality checks. |
| **Attribute** | A key-value pair on an emitted event. Attributes come from two sources: event-defined (struct fields via `#[derive(Event)]`) and enrichment. Both are subject to the same redaction rules and end up as key-value pairs on the exported record. |
| **Enrichment** | The process of attaching attributes to all events within a scope |
| **Event** | A Rust struct implementing the `Event` trait (via `#[derive(Event)]`). Represents a single telemetry occurrence with typed attributes, severity, and routing instructions. |
| **ProcessingInstructions** | Compile-time-generated rules that determine how an event is routed to logs, metrics, and traces. Contains `LogProcessingInstructions`, `MetricProcessingInstructions`, and trace instructions. |
| **RedactionEngine** | The `data_privacy` crate's engine for applying data-classification-aware redaction to field values. |
| **DataClass** | Describes the taxonomy of a type. Part of `data_privacy` crate. |
| **Fan-out** | The behavior of `emit!()` dispatching an event to all scoped sinks in the current context. |
