# observed - Telemetry Framework Design

`observed` is a collection of crates that provide a telemetry frontend solution.

> **How to read this document.** [Requirements](#requirements) state what the
> design aims for - not what ships today. [Technical details](#technical-details)
> describes the **implemented** contract. Anything designed but not yet built is
> collected under [Planned, not implemented](#planned-not-implemented). Where this
> document and the code disagree, the code wins: please fix the document.

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
   NOTE: logs and metrics ship today; the trace signal is
   [planned](#planned-not-implemented).

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
   NOTE: [planned](#planned-not-implemented) - today the compile-time defaults are final, and a processor can only accept or reject an event through
   `EventProcessor::is_interested`.

1. **Library and application telemetry isolation.** Libraries define the defaults for applications
   using that library, and on top of that, they can set up their own pipelines which diverge from
   those defaults if they want more or less detail.

1. **OpenTelemetry-native export.** Events are processed through standard `SdkLoggerProvider` and `SdkMeterProvider`, enabling any OTel-compatible exporter.

1. **Zero-cost when inactive.** Emitting must have zero overhead in the following scenarios:

- No exporters are configured.
- The event is disabled and no processor opts in to it.
- Low severity: severity pre-filtering skips event construction entirely - no field extraction, no allocation, no redaction call.
  Implemented in the processor: `EventProcessor::is_interested` runs before construction, and the `observed_destination`
  log processor answers it from the `OTel` logger's `event_enabled` check.
- Signal-level: if an emission results only in a metric, you should not pay the cost of logging - and vice versa.
  Partially implemented. Field *extraction* is lazy: a processor pulls only the fields it wants, so a field nobody visits costs no clone,
  no allocation, and no redaction call. Field *initialization* is not: once any processor declares interest, the whole event expression is
  evaluated, so an expression initializing a log-only field still runs even when every interested processor is metric-only.
  Deferring per-signal initialization is [planned](#planned-not-implemented).

1. **Enrichment crates.** A dedicated crate (e.g., `auth_attributes`, `http_context`) can define a public enrichment type reusable by other crates.

1. **Flat composition.** Multiple attribute sources (HTTP context, auth claims, tenant info,
   infrastructure metadata, tracing context) must compose without **nesting** closures.
   Five attribute providers should not produce five levels of indentation.

1. **Events and enrichment are runtime agnostic.** Everything should work independently of any
   async runtime. There might be some additional requirements for how to propagate context
   between tasks/threads.
   NOTE: That also includes propagation of native OTel `Context`/spans in case a library uses the `opentelemetry` crate directly.
   This part is [planned](#planned-not-implemented): `Sink::transfer_context` currently carries `observed`'s own enrichment chain and
   takes no dependency on OTel `Context`.

1. **Zero-cost bypass for fields that don't require redaction.** Fields annotated with `unredacted` skip the redaction engine entirely (no `RedactedDisplay` call, no allocation).
   Also applicable to fields that have a `DataClass` not requiring any redaction.

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
   blowing up the dependency tree of its users. It links neither `opentelemetry_sdk` nor
   `opentelemetry`: the `OpenTelemetry` representation lives in the exporter crates, so an
   `opentelemetry` version bump is not a breaking change for `observed` or its consumers.

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

   The *public API* names only traits that live in `data_privacy_core` - `Redactor` and
   `RedactedDisplay`. Field getters take `&dyn Redactor` rather than `&RedactionEngine`, so a
   processor may drive redaction with any redaction strategy and is not forced to build an engine.
   `data_privacy` itself is still needed for `Sensitive<T>`, which the `data_class = <expr>` form of
   `#[event(...)]` generates; that type has no `data_privacy_core` equivalent.

1. **Async propagation is manual in the core.** The `observed` crate itself is runtime agnostic, so async
   code must explicitly propagate enrichment context and library pipelines across `.await` points, via
   `Sink::transfer_context` plus `EnrichFutureExt::attach`. Runtime integrations layer automation on top:
   tasks spawned through `observed_rt` (over `anyspawn`) or `oxidizer_rt` inherit the spawn-site context
   without the caller attaching anything. Hand-attaching is therefore integration plumbing - spawners,
   `tower` layers and similar middleware - not everyday application code.
   See [Cross-thread enrichment transfer](#cross-thread-enrichment-transfer) for the composition contract.

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
   string interpolation that could enable injection attacks. This covers typed events; a
   `DynEvent` adaptor's body is runtime text that no `RedactionEngine` sees, so the adaptor
   must pre-sanitize it.

### Privacy

1. **Redaction-by-construction.** The core privacy guarantee: attribute values cannot reach an exporter without an explicit classification decision.
   Every field in `#[event(...)]` and `#[derive(Enrichment)]` follows one of three paths:
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

A **Sink** is a composable event dispatcher identified by a `SinkId`. It is the unit of telemetry configuration - each sink owns:

- One or more **processors** (`EventProcessor`), each targeting one destination and owning its own `RedactionEngine` and any OTel providers it needs.
- A **clock**, used to stamp every event it dispatches.
- An **enrichment slot** plus an **enrichment isolation** flag.

Sinks are plain values: construct one with `Sink::new`, clone it cheaply, and pass it around. `emit!()` always takes a sink as its first argument and
dispatches through that sink alone. To fan one `emit!()` out to several destinations, combine
sinks with `Sink::composite`, which flattens to a list of leaves and dispatches through each in turn.

Routing is decided per processor: the compile-time metadata generated by `#[event(...)]` is handed to `EventProcessor::is_interested`, and only the
processors that answered `true` receive the constructed event. It runs both as the construction gate and again while routing, so a processor may see
it more than once per emission. See [Sinks and Keys](#sinks-and-keys) for the full technical details.

### Signal Routing: How Events Become Logs and Metrics

An event can produce any combination of signals defined by its schema. The trace signal is [planned](#planned-not-implemented).

#### Logs

1. **A log record is produced only when a severity attribute is present.** When present, all attributes are included in logs by default, but there is a way to opt-out per field.

#### Metrics

1. **A metric is produced only when an event or an event field is marked as a metric type.** If there is no such attribute in an event, the event produces no metrics.

1. **Metric dimensions are strictly opt-in.** An event field or enrichment field becomes a metric
   dimension only when it is explicitly marked with `#[dimension(metric = "...")]`. Unmarked fields are
   never added as dimensions, even on metric-only events.

1. Enrichment fields opt into metric dimensions the same way event fields do - via `#[dimension(metric = "...")]`.
   The `#[derive(Enrichment)]` macro does not support instrument attributes (enrichment cannot *be* a
   metric value), only dimension opt-in.

1. **A metric value is never optional.** An instrument records a measurement on every emission, so its
   value field cannot be an `Option<T>`: the `#[if_none(...)]` placeholder for a `None` is a string, which
   is not a valid measurement. Optional data belongs on a metric *dimension*, where a placeholder is
   meaningful. This is a compile-time error.

1. An instance of a metric instrument is created automatically when a metric is emitted for the first time.
   Instruments are stored as thread-local instances (we need to check if OTel is already doing this).

1. Every processor has its own instance of a metric instrument that publishes results to its own target.

In summary, the routing is determined by annotations on the event struct:

| Scenario | Log? | Metric? |
| --- | --- | --- |
| Severity attribute only | Yes | No |
| Severity attribute + one instrument attribute | Yes | Yes (one instrument) |
| Severity attribute + multiple instrument attributes | Yes | Yes (per instrument) |
| Instrument attribute(s) only | No | Yes (per instrument) |
| Neither | No | No |

A signal-less event (no severity attribute and no instrument attribute) is accepted by the macro. It carries only its name, fields, and enrichment,
which is useful for events routed by a processor that selects on the event name rather than on a signal.

## Technical details

### Event properties

Each event uses per-signal attributes at the struct level; both the severity attribute and the instrument attributes are optional.

| Annotation | Effect |
| --- | --- |
| `#[event("...")]` | **Required.** Declares the canonical event name used for routing and identification. Add `disabled` (`#[event("...", disabled)]`) to mark the event as opt-in: the flag is surfaced to processors as `EventDescription::is_disabled` rather than enforced by the sink, and the processors shipped in `observed_destination` skip such events unless they explicitly opt in. |
| A severity attribute - `#[trace]`, `#[debug]`, `#[info]`, `#[warning]`, `#[error]`, or `#[fatal]` (optionally with a message: `#[info("...")]`) | Declares the event as a log. `name` defaults to the event name; the message is optional. |
| One of `#[counter(...)]`, `#[updown_counter(field, ...)]`, `#[gauge(field, ...)]`, `#[histogram(field, ...)]` | Declares an event-level metric instrument. `name` defaults to the event name. A bare `#[counter]` may be fieldless and records `1` per emission; the others require a leading positional field naming the struct field that supplies the metric value. |

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
| instrument `<field>` | Leading positional field name inside `#[counter(...)]` / `#[updown_counter(...)]` / `#[gauge(...)]` / `#[histogram(...)]`, naming the field whose value IS the metric value | The referenced field must not also be a metric dimension; it must be `#[unredacted]` and a numeric primitive `Value` can carry (`u128`/`i128` are unsupported); `#[counter(x)]` requires an unsigned integer, `#[updown_counter(x)]` a signed one, and `gauge`/`histogram` accept any supported numeric type including floats |
| `#[data_class(<expr>)]` | Data-classification expression; wraps the value in `Sensitive::new(value, expr)` before redaction | Mutually exclusive with `#[unredacted]` |
| `#[unredacted]` | Bypass redaction; the type must implement `Into<Value>` | Mutually exclusive with `#[data_class(...)]` |

### Sinks and Keys

#### SinkId

A `SinkId` is a lightweight, `Copy` identifier for a sink. It wraps a `&'static str` label and can be defined as a `const` / `static` item.
Two ids are equal when their labels are equal, comparing the string contents. A sink built as `Sink::new("app", …)` is therefore targetable by an
independently declared `static APP: SinkId = SinkId::new("app")`, which is what makes `enrich_for(&sink, APP, …)` usable across crate boundaries.

#### Sink

A **Sink** is a composable event dispatcher identified by a `SinkId`. It is the unit of telemetry configuration - each sink independently controls:

- **Processors** - a sink holds one or more `EventProcessor` values (there is no upper limit).
  When an event is emitted, every processor that declared interest handles it independently. This allows a single sink
  to send telemetry to multiple destinations simultaneously (e.g. stdout for development, ETW for production, an in-memory buffer for testing).
  Each processor owns its own `RedactionEngine`, so the same value can be redacted differently per destination.

- **Clock** - the `tick::SimpleClock` used to stamp dispatched events. All processors on one leaf see the same instant, and a frozen clock keeps
  tests deterministic.

- **Enrichment isolation** - By default, a sink receives both global enrichments (from `.enrich()`) and targeted enrichments (from `.enrich_for()`).
  When isolation is enabled, the sink only sees targeted enrichments addressed to its id.
  This lets library authors keep their internal telemetry independent of the hosting application's enrichment context.

#### Composite sinks
`Sink::composite` builds a dispatcher over several sinks. Nesting flattens at construction time into a single list of leaves. A leaf must appear
exactly once: cloned leaves share one enrichment slot, so a duplicate - listed twice or reached through two nested composites - would push and
restore the same thread-local chain twice and dispatch every event to those processors more than once, so `Sink::composite` panics on a duplicate
rather than silently repairing it. A composite has no identity of its own
(`id()` reports the `<composite>` sentinel) and holds no enrichment: records travel through each leaf's own processors and carry that leaf's
`SinkId`, redaction, and enrichment. `.enrich(&composite, …)` broadcasts to every leaf's slot.

#### How sinks, processors, and destinations relate

```text
Sink (identified by SinkId)
├─ enrichment slot + isolation flag
├─ clock
├─ EventProcessor #1  (owns its RedactionEngine and any OTel providers)
├─ EventProcessor #2  (same, different destination)
└─ ...

Sink::composite([sink_a, sink_b])
├─ leaf a  (sink_a's own id, processors, enrichment slot, clock)
└─ leaf b  (sink_b's own id, processors, enrichment slot, clock)
```

### Cross-thread enrichment transfer

`Sink::transfer_context` snapshots the current thread's enrichment chain into a sendable `Transfer`.
`EnrichFutureExt::attach` wraps a future so that snapshot is restored on every poll and removed again
before the future yields, which keeps it from leaking into unrelated tasks sharing the worker thread.

**Contract.** An integration that adds entries of its own should carry them *inside* the transfer, with
`Transfer::with_enrichment` (global) or `Transfer::with_enrichment_for` (visible only to one `SinkId`,
mirroring `enrich_for`). Entries carried this way are independent of wrapper order, so they survive the
future being boxed or attached again further out - both of which integrations do. Wrapping `enrich`
*around* an attached future is supported only for a single `attach` on a plain, non-boxed future;
other compositions are outside the guarantee and silently lose the entries.

**Technical details.** Applying a transfer *replaces* each captured slot's chain rather than layering
onto it - that is what stops a transferred scope inheriting the target thread's ambient context. So
`Enriched<Transferred<F>>` loses its entries: the inner transfer overwrites what the outer wrapper
pushed. The inherent `Transferred::enrich` / `enrich_for` restore the intuitive result by re-ordering
the wrappers, and win over the `EnrichFutureExt` blanket impl because a type's inherent methods are
searched before its traits. That selection is why the guarantee is narrow: it needs the receiver's
statically-known type to be exactly `Transferred<_>`, and it re-orders one wrapper deep, so nested
transfers, boxed or type-erased receivers, explicit trait dispatch, and generic `F: EnrichFutureExt`
receivers all fall back to the losing shape.

### Processing of emitted event

In v1, processing will be done on the same thread that calls `emit!`.

:::mermaid
flowchart TD
    emit["<b>Emit Site</b><br/>emit!(sink, HttpRequest { method, url, status, duration_ms })"]
    filter["<b>Interest pass</b><br/>EventProcessor::is_interested, before construction and again while routing"]
    dimensions["<b>Collect dimensions</b><br/>Event fields + enrichment from the sink's thread-local slot"]

    emit --> filter --> dimensions

    dimensions --> loop

    subgraph loop ["For each interested processor of the emitting sink"]
        redact["<b>Redaction</b><br/>Pull field values through the processor's own RedactionEngine"]
        redact --> etw["<b>ETW Provider</b><br/>Logs via ETW"]
        redact --> metrics["<b>Metrics Exporter</b><br/>Metric observations + dimensions"]
        redact --> file["<b>File Logger</b><br/>Structured log records to file"]
        redact --> stdout["<b>stdout</b><br/>Human-readable log output"]
    end
:::

## Planned, not implemented

The facilities below are part of the intended design but are **not** in the shipped API. They are listed here so the sections above can be read as a
description of what exists today.

| Facility | Intended behaviour | Today |
| --- | --- | --- |
| **Trace signal** | An event participates in a trace span alongside its log and metric signals. | Only log and metric signals exist. `EventDescription` has no trace metadata. |
| **Runtime signal overrides** | Compile-time signal defaults overridden at runtime. | Compile-time defaults are final; a processor can only accept or reject the whole event. |
| **Native OTel `Context` propagation** | Enrichment interoperates with OTel `Context`/spans for libraries using `opentelemetry` directly. | `Sink::transfer_context` captures only `observed`'s own enrichment chain; there is no OTel `Context` dependency. |
| **Per-signal lazy initialization** | An emission that produces only a metric pays none of the cost of the log-only payload, including the expressions that initialize log-only fields. | The whole event expression is evaluated as soon as any processor is interested. Only field *extraction* is per-field lazy. |
| **Automatic enrichment scopes** | Enrichment attaches automatically when crossing crate boundaries. | Every scope is opened explicitly with `.enrich()` / `.enrich_for()`. |

## Appendix #1: supported data types

Values are exporter-neutral; each exporter maps these to its own wire format.

| Base type | `Value` variant |
| --- | --- |
| `bool` | `Bool` |
| `i32` / `i64` | `I64` |
| `u32` | `I64` |
| `f32` / `f64` | `F64` |
| `String` / `&'static str` / `Arc<str>` / `Cow<'static, str>` | `String` |
| `Vec<bool>` | `BoolArray` |
| `Vec<i64>` | `I64Array` |
| `Vec<f64>` | `F64Array` |
| `Vec<String>` / `Vec<&'static str>` | `StringArray` |

## Appendix #2: Acronyms and Definitions

| Term | Definition |
| --- | --- |
| **ETW** | Event Tracing for Windows - a high-performance kernel-level tracing facility. |
| **Sink** | A named event dispatcher identified by a `SinkId`. Holds one or more `EventProcessor` values, a clock, and enrichment configuration (slot, isolation flag). |
| **SinkId** | A `&'static str` label identifying a sink. Two ids are equal when their labels are equal. |
| **Composite sink** | A `Sink` built by `Sink::composite` that dispatches one `emit!()` through several leaf sinks, each keeping its own id, processors, and enrichment. |
| **Attribute** | A key-value pair on an emitted event. Attributes come from two sources: event-defined (struct fields via `#[event(...)]`) and enrichment. Both are subject to the same redaction rules and end up as key-value pairs on the exported record. |
| **Enrichment** | The process of attaching attributes to all events within a scope |
| **Event** | A Rust struct implementing the `Event` trait (via `#[event(...)]`). Represents a single telemetry occurrence with typed attributes, optional severity, and routing metadata. |
| **EventDescription** | The compile-time metadata constant generated by `#[event(...)]`: event name, type id, log and metric signal descriptions, and the `disabled` flag. Handed to `EventProcessor::is_interested` to decide routing. |
| **EventProcessor** | A destination-facing handler that declares interest in an event and then pulls the fields it needs from an `EventView`, redacting them through its own engine. |
| **RedactionEngine** | The `data_privacy` crate's engine for applying data-classification-aware redaction to field values. Implements `Redactor`. |
| **Redactor** | The `data_privacy_core` trait that applies redaction for a given data class. Field getters take `&dyn Redactor`, so any redaction strategy works, not only a full engine. |
| **DataClass** | Describes the taxonomy of a type. Part of `data_privacy` crate. |
| **Fan-out** | Dispatching one `emit!()` to several destinations, done explicitly via `Sink::composite`. |
