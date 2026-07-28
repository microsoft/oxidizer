<!-- Copyright (c) Microsoft Corporation. Licensed under the MIT License. -->

# Builder and transport abstraction

This document defines the builder architecture the WinHTTP work adopts. It is a
cleanup of the `fetch` configuration/transport seam, motivated by adding a second
real transport plus a concrete usability requirement (below): the current model
routes a single hyper-shaped options struct to every backend, so a
differently-shaped transport (WinHTTP) cannot be configured without
approximation.

It pairs with [`configuration-model.md`](configuration-model.md): that doc defines
*what* the configuration categories are and how each knob is bucketed; this doc
defines *how those buckets are surfaced* on the client builder and the transport.

## Requirement: libraries expose REST clients over `fetch`

The decisive constraint is that a library may wrap `fetch` to expose a REST
endpoint to its own Rust consumers. That creates two parties with different jobs:

- **The library** configures pipeline behavior it owns (e.g. resilience policy),
  and may want to accept a builder from the outside and layer its own config on
  top.
- **The consumer** preconfigures the client the library will use: it picks the
  transport (WinHTTP vs hyper), may set portable transport knobs (e.g. connect
  timeout), and must be able to inject a **mock** for tests.

Both `HttpClient` and its builder must therefore be **easy to name, pass, and
store** across a library boundary — without viral type parameters.

## Decision: concrete builder AND concrete client

`HttpClient` and `HttpClientBuilder` are both **single concrete, non-generic
types** (as they are today). The transport lives behind an erased trait object.

This reverses an earlier proposal to make `HttpClientBuilder<T: Transport>`
generic over the transport. That approach was rejected precisely because of the
requirement above:

- **A library can't accept a generic builder without becoming generic itself.**
  `fn configure<T: Transport>(b: HttpClientBuilder<T>)` is viral: the library's
  own wrapper types become `MyClient<T>`, and it cannot store the builder in a
  non-generic field.
- **A mock is a different type.** A real client is `…<WinHttpTransport>`, a mock
  is `…<FakeTransport>`; a non-generic library API cannot accept "either" without
  going generic again.
- The one thing the generic builder bought — compile-time transport-specific
  setters *on the client builder* — turns out to be the least important property
  here, and is recovered by putting that config on the transport's own builder
  (below).

## The `Transport` seam (object-safe)

A dynamically-dispatched trait replaces the concrete `Transport` grab-bag in
`fetch::custom`. It is object-safe so `HttpClientBuilder` can hold
`Box<dyn Transport>` (or the existing `Arc`-wrapped erased factory) and stay
non-generic:

```rust
/// A transport turns a fully-formed request into a response, honoring the
/// portable options fetch guarantees to every transport.
pub trait Transport: ThreadAware + Send + Sync + 'static {
    /// Build the leaf handler for one dispatch slot. MUST honor the "required"
    /// portable knobs (see the bucket model) or return an error at build time —
    /// never silently ignore a required knob.
    fn connect(&self, portable: &PortableOptions, cx: TransportContext)
        -> Result<TransportHandler>;
}
```

`HyperTransport`, `WinHttpTransport`, and a `FakeTransport` (mock) all implement
it. The existing per-slot plumbing (`create_transport_handler`, invoked once per
dispatch slot, per core under `Isolation::Isolated`) is unchanged — `connect` is
its typed-input form. `connect` is fallible so a transport can fail fast at build
when it cannot honor a required or security-relevant knob.

## The knob bucket model

Every configuration knob sorts into exactly one bucket. The bucket decides *where*
the knob is set and *who* can set it. (The per-knob mapping onto WinHTTP lives in
[`configuration-mapping.md`](configuration-mapping.md); the concrete WinHTTP
option inventory is in [`winhttp-capabilities.md`](winhttp-capabilities.md).)

| Bucket | Where it's set | Who sets it | Honored by the transport? |
| --- | --- | --- | --- |
| **A — Pipeline** | `HttpClientBuilder` methods; consumed *above* the transport | library or consumer | No transport involvement |
| **B — Required portable** | `HttpClientBuilder` methods → pushed via `PortableOptions` | library or consumer | Every transport MUST honor (or fail fast) |
| **B2 — Optional portable** | `HttpClientBuilder` methods → pushed via `PortableOptions` | library or consumer | Capability-gated: honor if able; security knobs fail fast, perf knobs may document a coarser/no-op behavior |
| **C — Transport-specific** | the transport's **own** builder, before erasure | consumer only | Honored natively by that one transport |

### Bucket A — pipeline-owned (portable, no transport cooperation)

Enforced by pipeline layers above the leaf, so no transport ever sees them:
scheme policy (`request_filter` — enforced in `Dispatch::validate`), resilience
(retry/hedge/breaker/timeout via `seatbelt`), telemetry/metrics/logging, base
URI/router, redaction, response-body buffering/idle limits, and multi-pool
`(count, selection)` (which runs *N* transport handlers and load-balances — a
`fetch` concept distinct from a transport's internal connection pool).

### Bucket B — the minimal required contract

The small set every transport must honor, pushed to `connect` via
`PortableOptions`:

```rust
#[non_exhaustive]
pub struct PortableOptions {
    pub connect_timeout: Duration,
    pub http_versions: Vec<Version>,        // preference; may include HTTP/3
    pub client_identity: Option<ClientIdentity>, // mTLS; honor-or-fail-fast
    pub max_connections_per_host: Option<u32>,
    pub extra: Extensions,                  // per-request escape hatch
    // ... plus behavioral guarantees not expressed as fields:
    //   * streaming request/response bodies
    //   * cancellation on future-drop
}
// NOTE: scheme policy is deliberately absent — Dispatch owns it (Bucket A).
```

### Bucket B2 — optional, capability-gated portable knobs

Portable knobs a **library** may legitimately need but not every transport can
honor identically. They live on `PortableOptions` too, but each carries an
explicit capability contract:

- **certificate pinning / validation policy** — *security semantics*: honor, or
  **fail fast** at build if the transport cannot pin. (Both transports can:
  hyper via a custom verifier; WinHTTP by inspecting
  `WINHTTP_OPTION_SERVER_CERT_CONTEXT` in the status callback.)
- **connection idle timeout** and **max lifetime** — *perf semantics*: honored
  natively where possible (WinHTTP has a native max-lifetime primitive and a
  built-in idle scavenger; see `winhttp-capabilities.md`), with any granularity
  gap documented rather than silently dropped.
- **coarse keep-alive** (on/off + idle-triggered health check) — perf semantics;
  maps to WinHTTP's `WINHTTP_OPTION_HTTP2_KEEPALIVE` for the idle case.

Revocation is **not** a knob: every transport checks revocation unconditionally
(an always-on invariant), so it never appears in any bucket.

### Bucket C — transport-specific (consumer-only)

Configured on the transport's own builder, before it is erased into
`Box<dyn Transport>`. Never on the client builder:

- hyper: connection-pool internals/poisoning, keep-alive PING probe *timings*,
  HTTP/2 stream tuning, userspace TLS backend selection + custom verifier beyond
  the portable pinning hook.
- WinHTTP: proxy/WPAD, integrated Windows auth, per-server tuning beyond the
  portable cap, SChannel-specific options.

## How the parties compose

```rust
// Consumer picks + configures the transport (Bucket C, type-safe on its builder):
let transport = WinHttpTransport::builder(deps).proxy(proxy).build();
//   ...or HyperTransport::builder(...).connection_pool_options(...).build();
//   ...or FakeTransport::new(canned_responses)   // a mock is just a Transport

// Concrete, non-generic builder — this is what a library accepts:
let builder: HttpClientBuilder = HttpClient::builder(transport);

// Library layers its portable config (Bucket A + B/B2) on the concrete builder:
fn configure(b: HttpClientBuilder) -> HttpClient {
    b.retry(/* ... */)               // A
     .connect_timeout(/* ... */)     // B
     .connection_idle_timeout(/* */) // B2
     .build()
}
```

This satisfies every part of the requirement:

- **Library configures resilience + portable knobs** — all Bucket A/B/B2 methods
  exist on the concrete `HttpClientBuilder`; `fn configure(HttpClientBuilder)` is
  non-generic.
- **Consumer preconfigures the transport, incl. transport-specific knobs** — done
  on the transport's own builder (Bucket C), then handed to the library as a
  ready `HttpClientBuilder`.
- **Mock** — `HttpClient::builder(FakeTransport::new(...))`; the library's
  `configure(HttpClientBuilder)` accepts it unchanged. This is the same
  substitution the existing `builder_fake` path already uses.
- **Nameable client and builder types** — both are concrete; a library can store
  or return either without type parameters.

### Escape hatch for the rare case

If a library genuinely must touch a Bucket-C knob it does not own, a typed
downcast is available:

```rust
fn transport_config_mut<C: 'static>(&mut self) -> Option<&mut C>;
```

This is deliberately awkward (the library must know the concrete config type) so
the encouraged path stays: Bucket C is the consumer's responsibility.

## Backwards compatibility

Because both types stay concrete, the blast radius is small and there is no viral
type parameter.

- Most of today's `HttpClientBuilder` methods (`connect_timeout`,
  `insecure_allow_http`, `supported_http_versions`, the resilience/pipeline
  methods) are Bucket A/B and **keep working unchanged**.
- Parts of `connection_pool_options` decompose across buckets: `max_connections`
  → B, idle-timeout/max-lifetime → B2 — all of which remain portable methods on
  the concrete builder.
- The genuinely hyper-specific remainder (`http2_options` stream tuning,
  keep-alive PING *timings*, pool poisoning internals) is Bucket C and moves to
  `HyperTransport::builder()`. For source compatibility, the existing
  `HttpClientBuilder::{http2_options, connection_keep_alive, connection_pool_options}`
  methods are retained as **`#[deprecated]` shims** that forward into the hyper
  transport's config (carried on the erased transport), no-ops for non-hyper
  transports. They can be removed in a later major version.
- **`HttpClient` is untouched**, so everything downstream of `build()` is
  source-compatible.

## Blast radius

- **`fetch`**: `Transport` becomes an object-safe trait; `HttpClientBuilder` gains
  Bucket A/B/B2 methods and a `PortableOptions`; the hyper-specific setters become
  deprecated shims. `HttpClientBuilder` and `HttpClient` stay concrete.
- **`fetch_options`**: `TransportOptions` shrinks toward `PortableOptions`;
  `ConnectionPoolOptions` / `ConnectionKeepAlive` / `Http2Options` split — the
  portable parts stay, the hyper-specific parts move to `fetch_hyper`.
- **`fetch_hyper`**: implements `Transport`; owns its Bucket-C config on its own
  builder. **`fetch_winhttp`**: implements `Transport`; owns `WinHttpOptions`
  (Bucket C) on its own builder.
- **`fetch_m365`**: `FetchExt` constructors return the concrete
  `HttpClientBuilder` (no signature change from a type parameter).
- **`fetch_azure`**: does not consume the moved option types directly; expected
  unaffected.

Each dependent follows the workspace release cascade (`docs/releasing.md`).
