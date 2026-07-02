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
  targets, and factors is fixed when the provider is built and never changes for
  the life of the provider.
- **Zero cost where unused.** Scopes and instruments that are not configured for
  rescaling incur no wrapping and delegate directly to the inner provider.

## Usage shape

```rust,ignore
let inner = build_inner_meter_provider();

let outer = RescaledMetrics::builder(inner)
    .scope("my_scope_name", |scope| {
        scope.rescale("http.client.request.duration",
                      "http.client.request.duration.millis",
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
A consequence is that the callbacks run **twice per collection** — see open
questions.

## Configuration model

Configuration is a map from scope to a set of rescale rules; each rule maps a
source instrument name to one or more targets, each with its own factor. A single
source may therefore feed several sidecars. Matching a source instrument is by
name within its scope; the same rule applies to whichever value type the caller
happens to build under that name.

Validation happens at build time so misconfiguration fails fast rather than
silently dropping metrics.

## Relationship to the wider system

The crate depends only on the `opentelemetry` API crate — not on
`opentelemetry_sdk` — so it composes with any conforming provider, including the
SDK provider, the no-op provider, and other wrappers. It is itself a
`MeterProvider`, so wrappers may be stacked.

## Open questions

These need decisions before (or during) implementation:

1. **Integer rescaling & rounding.** The factor is `f64`, but counters and gauges
   may be `u64`/`i64`. What conversion applies — round, truncate, saturating?
   How are overflow and negative-from-unsigned handled? A factor of `1000.0` on a
   `u64` value near the type maximum overflows.
2. **Histogram bucket boundaries.** If values are scaled by the factor, the
   sidecar's bucket boundaries must be scaled by the same factor, otherwise every
   measurement collapses into one bucket. Should the layer auto-scale explicit
   boundaries, and what does it do when the source used the SDK's default
   boundaries (which it cannot see)?
3. **Sidecar metadata.** Should the sidecar inherit the source's description and
   unit verbatim, or should the configuration allow overriding them (e.g. unit
   `s` → `ms`)? Inheriting a stale unit is arguably misleading.
4. **Observable double invocation.** Registering the user callbacks twice means
   they execute twice per collection. Is that acceptable, or do we need a caching
   strategy (observe once, replay scaled) with its own correctness caveats?
5. **Scope identity.** Match scopes by name only, or by the full instrumentation
   scope (name + version + schema URL + attributes)? Name-only is simpler but can
   over-match when several libraries share a scope name.
6. **Transform generality.** Is a single multiplicative factor sufficient, or do
   we anticipate needing affine (offset) or arbitrary transforms? This shapes the
   configuration type even if only multiplication ships first.
7. **Target name collisions.** What happens if a sidecar's target name equals an
   instrument the caller also creates directly, or another sidecar's target? The
   SDK would see duplicate registrations. Do we detect and reject, or document?
8. **Config validation rules.** Which configurations are rejected at build time —
   factor of `0.0`, `NaN`/infinite factors, negative factors, `source == target`,
   duplicate targets within a scope?
9. **Inner provider ownership.** Is the inner provider taken by value (generic
   over its type, zero-cost) or type-erased behind a trait object? This affects
   ergonomics for callers who need to retain their own reference to the inner
   provider.
10. **Allowed external types.** The public API will expose `opentelemetry` types
    (`MeterProvider`, `Meter`, `KeyValue`, …); these must be enumerated in the
    crate's `allowed_external_types` allowlist, as sibling crates do.
