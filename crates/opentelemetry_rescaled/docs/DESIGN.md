# Opentelemetry Rescaled — Architecture & Design

This document describes the intended design of the crate so it can be reviewed
before implementation. For the user-facing summary see the crate-level rustdoc
(`src/lib.rs`).

## Goal

Provide a wrapping layer around an existing OpenTelemetry meter provider that,
for specific instruments in specific instrumentation scopes, transparently emits
a **rescaled sidecar** of each such instrument: a second instrument carrying the
same measurements multiplied by a fixed factor.

The canonical motivating case: an instrument records a duration in seconds
(`http.client.request.duration`) and a downstream system expects milliseconds.
Rather than change the instrumented code, the operator configures a sidecar
`http.client.request.duration.millis` with factor `1000.0`. Both instruments are
exported independently by the underlying SDK.

Scope: **metrics only**, all instrument kinds (synchronous and observable, for
every value type the API supports).

## Tenets

- **Transparent to the measurer.** Code recording measurements sees exactly one
  instrument — its original. It has no knowledge that a sidecar exists, and its
  hot path is unchanged apart from the fan-out described below.
- **Transparent to the inner provider.** The wrapped SDK sees two ordinary,
  independently registered instruments. No SDK internals are touched; the layer
  composes purely through the public OpenTelemetry API surface.
- **Configured once, at build time.** The set of scopes, source instruments,
  targets, units, and factors is fixed when the provider is built and never
  changes for the life of the provider.
- **Zero cost where unused.** Scopes and instruments that are not configured for
  rescaling incur no wrapping and delegate directly to the inner provider.
- **Fail fast on nonsense.** A configuration that cannot produce a meaningful
  sidecar (see [Configuration model](#configuration-model)) panics at build time
  rather than silently emitting garbage.

## Usage shape

```rust,ignore
let inner = build_inner_meter_provider();

let outer = RescaledMetrics::builder(inner)
    .scope("my_scope_name", |scope| {
        // source name, target name, target unit (mandatory), factor
        scope.rescale("http.client.request.duration",
                      "http.client.request.duration.millis",
                      "ms",
                      1000.0);
    })
    .build();

// `outer` is itself a `MeterProvider`; hand it wherever the inner one went
// (e.g. to instrumented libraries, or `global::set_meter_provider`).
```

## How interception works

OpenTelemetry's Rust metrics API is layered as
`MeterProvider` → `Meter` → typed instrument builders → concrete instruments, and
a `Meter` is nothing more than a handle to an `InstrumentProvider`. This layering
is the seam the crate exploits: it substitutes its own `MeterProvider` and
`InstrumentProvider` while delegating all real work to the inner ones.

### The provider wrapper

`RescaledMetrics` implements `MeterProvider`. When a scoped meter is requested it
resolves the inner scoped meter and then decides, by matching the scope against
the configuration:

- **Unconfigured scope** → return the inner meter unchanged (no wrapping, no
  overhead).
- **Configured scope** → return a meter backed by a *rescaling instrument
  provider* that holds the resolved inner meter plus that scope's rescale rules.

### Synchronous instruments — fan-out

A synchronous instrument (`Counter`, `UpDownCounter`, `Gauge`, `Histogram`)
delegates every measurement to an inner `SyncInstrument`. The rescaling provider
constructs, for a configured source instrument, **two** inner instruments — the
original and the sidecar — and returns to the caller a single instrument whose
backing `SyncInstrument` is a small **fan-out**:

```text
caller.add(v, attrs)
        │
        ▼
   fan-out.measure(v, attrs)
        ├─────────────► original.measure(v,            attrs)
        └─────────────► sidecar .measure(scale(v),     attrs)
```

The caller holds one handle; each recorded measurement reaches both inner
instruments. Because the fan-out records through the ordinary instrument handles,
no SDK internals are involved.

### Observable instruments — dual registration

Observable instruments (`ObservableCounter`, `ObservableUpDownCounter`,
`ObservableGauge`) carry user callbacks that the SDK invokes at collection time,
passing an observer bound to one specific instrument. There is no public
multi-instrument callback, so the layer registers the source instrument's
callbacks **twice** on the inner meter:

- once on the original instrument, invoking the callbacks with the observer as-is;
- once on the sidecar instrument, invoking the same callbacks through a **scaling
  observer** that multiplies every observed value before forwarding it.

The user's callbacks are shared (behind an `Arc`) between the two registrations.
A consequence is that the callbacks run **twice per collection**. This is an
accepted cost: callbacks are expected to be cheap and idempotent, and a
replay-once cache would introduce its own staleness and correctness hazards for
no meaningful benefit.

## Value rescaling

A rescale factor is always a plain multiplicative `f64` — the only transform the
crate needs. Applying it depends on the instrument's value type:

- **`f64` instruments** multiply directly.
- **`u64`/`i64` instruments** multiply in `f64`, **round** to the nearest
  integer, and **saturate** at the type's bounds. Saturation rather than
  wrap/overflow keeps a runaway sidecar bounded and obvious instead of silently
  corrupt.

Histograms need one extra step: the sidecar's **bucket boundaries** are scaled by
the same factor as the values, so the buckets stay meaningful. When the source
instrument supplies explicit boundaries the layer scales them; when it relies on
the SDK's default boundaries there is nothing to scale (the defaults are not
visible through the API), so the sidecar simply keeps the defaults. That yields
an obviously wrong bucketing that prompts the operator to configure real
boundaries — acceptable because default boundaries are not expected in real
production use.

## Configuration model

Configuration is a map from scope to a set of rescale rules. Each rule maps a
source instrument name to one or more targets; a target carries its **name**, its
**unit** (mandatory), and its **factor**. A single source may therefore feed
several sidecars. The sidecar inherits the source's description but **must** be
given a new unit at configuration time — rescaling almost always changes the unit
(`s` → `ms`), and inheriting the stale one would be misleading.

Matching a source instrument is by name within its scope, and the same rule
applies to whichever value type the caller builds under that name.

Scopes are matched **by name only** for now. If several instrumentation scopes
share a name, the rules apply to all of them. The configuration type is shaped so
that stricter matching (e.g. a future `scope_exact(...)` keyed on the full
instrumentation scope — name, version, schema URL, attributes) can be added later
without reworking the model.

Duplicate target names across the process are **not** the crate's concern:
duplicate instrument registration is always possible in OpenTelemetry, and it is
the user's job to avoid collisions and the SDK's job to cope with them.

Validation happens at build time, and a configuration that cannot yield a
meaningful sidecar **panics** — for example a factor that is `0.0`, `NaN`,
infinite, or negative, a rule whose source equals its target, or duplicate
targets within a scope.

## Relationship to the wider system

The crate depends only on the `opentelemetry` API crate — not on
`opentelemetry_sdk` — so it composes with any conforming provider, including the
SDK provider, the no-op provider, and other wrappers. It is itself a
`MeterProvider`, so wrappers may be stacked.

The inner provider is taken **by value** but immediately **type-erased** behind a
trait object, so `RescaledMetrics` carries no generic parameter for it and does
not leak the inner provider's concrete type into callers' signatures.

The public API surface exposes `opentelemetry` types (`MeterProvider`, `Meter`,
`KeyValue`, …); these are enumerated in the crate's `allowed_external_types`
allowlist, as sibling crates do.
