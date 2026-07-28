<!-- Copyright (c) Microsoft Corporation. Licensed under the MIT License. -->

# Architecture

## Where the crate sits

```
                 fetch::HttpClient  (unchanged public API)
                         │
          ┌──────────────┴───────────────┐
          │  pipeline (transport-agnostic) │  buffering, metrics, logging,
          │                                │  retry, hedging, circuit breaking
          └──────────────┬───────────────┘
                         │  leaf = TransportHandler
                         │        = Service<HttpRequest, Result<HttpResponse>>
          ┌──────────────┼───────────────┐
          │              │               │
   fetch_hyper     fetch_winhttp     <fake, custom, ...>
  (hyper+TLS)      (this crate)
```

`fetch_winhttp` is a *peer* of `fetch_hyper`: both produce a leaf
`RequestHandler`. It does **not** depend on `fetch` (avoids a dependency cycle);
it depends only on the shared vocabulary crates (`http`, `http_extensions`,
`fetch_options`, `fetch_tls`, `bytesbuf`, `tick`, `ohno`, `opentelemetry`), the
`plurality` object pool (for per-request state; see
[`async-bridge.md`](async-bridge.md#allocating-requestshared-from-a-plurality-pool)),
and the WinHTTP FFI. `fetch` and `fetch_m365` depend on `fetch_winhttp` and do
the wiring.

This mirrors exactly how `fetch_hyper` relates to `fetch` today.

## The integration contract

`fetch` exposes the plug-in point:

```rust
// fetch::custom
pub fn create_builder<F, R, Extras>(
    runtime: impl Into<Cow<'static, str>>,   // telemetry: fetch.runtime
    transport: impl Into<Cow<'static, str>>, // telemetry: fetch.transport
    factory: F,                              // Fn(CustomContext<Extras>) -> R
    isolation: Isolation,                    // Shared | Isolated
    deps: impl Into<CustomDeps<Extras>>,
) -> HttpClientBuilder
where
    F: Fn(CustomContext<Extras>) -> R + Send + Sync + 'static,
    R: RequestHandler + 'static,             // <-- our transport
    ...;
```

The `factory` is invoked once per connection-pool slot. It receives a
`CustomContext` carrying everything a transport needs to configure itself:

| `CustomContext` field | How `fetch_winhttp` uses it |
| --- | --- |
| `options: TransportOptions` | The **generic** surface only (connect timeout, scheme filter, HTTP-version preference). Hyper-shaped pool/keep-alive/HTTP-2 knobs no longer live here — see [`configuration-model.md`](configuration-model.md). |
| `tls: TlsOptions` | Universal core: client identity (mTLS) and version/ALPN preference. See the TLS section of the mapping doc. |
| `body_builder: HttpBodyBuilder` | Wrap response bodies so they draw from the client's memory pool (`bytesbuf`), keeping usage-neutral accounting consistent with other transports. |
| `clock: Clock` | Timeout accounting / any timers the transport needs. |
| `pool_index: PoolIndex` | Tag connection-level telemetry. |
| `meter: Meter` | Emit transport-level metrics under the client's meter scope. |
| `extras` | Carries transport dependencies (not user config) — for `fetch_m365`, any extra deps. Transport-specific *config* (`WinHttpOptions`, Bucket C) is set on `WinHttpTransport`'s own builder; see [`builder-architecture.md`](builder-architecture.md). |

Because `create_builder` returns a normal `HttpClientBuilder`, the full pipeline
is layered on top automatically. **This is what makes the user experience
transparent.**

## Public API shape

The crate follows `fetch_hyper`'s builder → type-erased-transport pattern, minus
the `Connect`/stream generics (WinHTTP owns the connection, so there is no
caller-supplied connector and nothing to be generic over).

```rust
// fetch_winhttp — proposed surface (Windows only)

/// A type-erased WinHTTP request handler. Implements
/// `Service<HttpRequest, Result<HttpResponse>>` (i.e. `RequestHandler`).
pub struct WinHttpTransport { /* ... */ }

impl Service<HttpRequest> for WinHttpTransport {
    type Out = Result<HttpResponse>;
    fn execute(&self, request: HttpRequest) -> impl Future<Output = Self::Out> + Send;
}

/// Builder for `WinHttpTransport`. `PortableOptions` supply the portable
/// (Bucket B/B2) knobs pushed from the `fetch` pipeline; `WinHttpOptions` supply
/// the WinHTTP-native (Bucket C) ones set directly by the consumer. TLS config is
/// derived from `TlsOptions`. See [`builder-architecture.md`](builder-architecture.md).
pub struct WinHttpTransportBuilder { /* ... */ }

impl WinHttpTransportBuilder {
    /// `winhttp` carries the WinHTTP-native, consumer-set configuration (proxy,
    /// integrated auth, per-server tuning). Portable options arrive later via the
    /// `Transport::connect` contract, not here. See `configuration-model.md`.
    pub fn new(clock: Clock, winhttp: WinHttpOptions) -> Self;

    pub fn body_builder(self, body_builder: HttpBodyBuilder) -> Self;
    pub fn pool_index(self, pool_index: PoolIndex) -> Self;
    pub fn meter(self, meter: Meter) -> Self;

    /// Produces the erased `WinHttpTransport` (a `dyn Transport`). Portable knobs
    /// (incl. mTLS identity) are honored per request at `Transport::connect`;
    /// construction fails fast for a security knob WinHTTP cannot honor.
    pub fn build(self) -> WinHttpTransport;
}

/// Transport-specific (Bucket C) options owned by this crate — the WinHTTP-native
/// knobs that have no portable `fetch_options` home. Set on the transport's own
/// builder by the consumer; revocation is NOT here (it is an always-on invariant).
pub struct WinHttpOptions { /* proxy, integrated_auth, ... */ }
```

Note the deliberate differences from `HyperTransportBuilder`:

- **No `Spawner`.** WinHTTP async mode does its own I/O on OS threads and calls
  back; there is no hyper background task to drive, so no runtime executor is
  required. See [`async-bridge.md`](async-bridge.md).
- **No `Connect<S>` generic.** WinHTTP establishes connections itself.
- **`build` is fallible.** `fetch_hyper::build` is infallible because rustls
  backend construction happened earlier; here, resolving `TlsOptions` into a
  WinHTTP-expressible configuration can legitimately fail (e.g. an unsupported
  knob), and we surface that eagerly at build time.

## Factory wiring in `fetch` (opt-in)

A thin, Windows-gated feature (`winhttp`) adds constructors that mirror
`builder_tokio` / `new_tokio`:

```rust
// fetch (feature = "winhttp", cfg(windows)) — proposed
impl HttpClient {
    /// Returns a concrete `HttpClientBuilder` backed by the WinHTTP transport.
    /// Transport-specific (Bucket C) knobs are set on `WinHttpTransport::builder`
    /// before this call; portable knobs are set on the returned builder. See
    /// builder-architecture.md.
    pub fn builder_winhttp(
        deps: impl Into<WinHttpDeps>,
    ) -> HttpClientBuilder {
        // Constructs a WinHttpTransport with default WinHttpOptions and hands it
        // to HttpClient::builder(transport). For Bucket-C configuration, build the
        // transport yourself: HttpClient::builder(WinHttpTransport::builder(deps).proxy(..).build()).
    }
    pub fn new_winhttp() -> Self;
}
```

`WinHttpDeps` carries a `Clock` and a `GlobalPool`, matching `TokioDeps`.

## Factory wiring in `fetch_m365`

A new `winhttp` module adds a `FetchExt` trait alongside the existing
`tokio::FetchExt` / `oxidizer::FetchExt`:

```rust
// fetch_m365::winhttp (feature = "winhttp", cfg(windows)) — proposed
pub trait FetchExt {
    fn builder_m365_winhttp(deps: impl Into<WinHttpDeps>) -> HttpClientBuilder;
    fn new_m365_winhttp(deps: impl Into<WinHttpDeps>) -> HttpClient { /* build() */ }
}
impl FetchExt for HttpClient { /* ... */ }
```

The M365 variant differs from the base `fetch` variant only in the M365 policy
it applies on top. For certificate validation, M365 policy accepts WinHTTP's
OS-native (SChannel) trust-chain validation in place of the SymCrypt-backed
`oxidizer_security` `Validator` used by the hyper/rustls transports, so
`builder_m365_winhttp` does not wire up that `Validator` (see the mapping doc's
TLS section).

## Crate layout (planned)

Mirrors `fetch_hyper` so contributors moving between the two transports find the
same shape.

```
crates/fetch_winhttp/
├── Cargo.toml
├── README.md                 # auto-generated from lib.rs by `just readme`
├── docs/design/              # this folder
└── src/
    ├── lib.rs                # crate docs + re-exports (skeleton today)
    ├── builder.rs            # WinHttpTransportBuilder, WinHttpTransport
    ├── options.rs            # WinHttpOptions (Bucket C) + PortableOptions
    │                         #   translation to WinHttpSetTimeouts / WinHttpSetOption
    ├── session.rs            # WINHTTP session/connection handle lifecycle (RAII)
    ├── request.rs            # per-request handle: send, receive, body pump
    ├── async_bridge.rs       # status-callback → Future adapter (see async-bridge.md)
    ├── tls.rs                # TlsOptions → WinHTTP TLS/cert config (WinHttpTls)
    ├── error_labels.rs       # stable telemetry labels (mirror fetch_hyper)
    ├── recoverability.rs     # WinHTTP error → recoverable::Recovery classification
    ├── telemetry.rs          # connection/request metrics via the shared Meter
    └── ffi/                  # windows-sys bindings, safe RAII handle wrappers
```

## `unsafe` / FFI policy

WinHTTP is a C API, so FFI `unsafe` is unavoidable — this is the one place the
crate diverges from the workspace's default no-`unsafe` guidance. It is
justified and contained:

- All FFI lives under `src/ffi/`. Every `unsafe` block carries a
  `// SAFETY:` comment (required by the `clippy::undocumented_unsafe_blocks`
  workspace lint) explaining the precondition being upheld.
- Raw handles (`HINTERNET`) are wrapped in RAII newtypes whose `Drop` calls
  `WinHttpCloseHandle`, so the rest of the crate is safe, leak-free Rust.
- `windows-sys` (already a workspace dependency, v0.61) provides the bindings;
  we do not hand-roll `extern` declarations. Enable the
  `Win32_Networking_WinHttp` and `Win32_Foundation` features.
- Precedent for contained, documented `unsafe` in this workspace exists in
  `bytesbuf`, `multitude`, and `plurality`.

### FFI-crate decision: `windows-sys` over `windows`

We use **`windows-sys`**, not the higher-level `windows` crate. Both are
generated from the same win32metadata, so their WinHTTP coverage is
**identical** — the full namespace (`HINTERNET`, the `WinHttp*` functions,
`WinHttpSetStatusCallback` / `WINHTTP_STATUS_CALLBACK`, `WINHTTP_FLAG_ASYNC`,
`WINHTTP_PROTOCOL_FLAG_HTTP2` / `HTTP3`, `WINHTTP_OPTION_*`). The choice is about
weight versus ergonomics:

- **Convention / weight.** This crate lives in **oxidizer2**, whose only Windows
  binding is `windows-sys` (used by `bytesbuf`); the `windows` crate is not an
  oxidizer2 workspace dependency. Adopting `windows` would introduce a heavier
  new dependency graph (`windows-core` and satellites) for marginal gain. (The
  `windows` crate *is* available in ox-sdk2, where `fetch_m365` does its wiring —
  but the transport crate itself is here in oxidizer2.)
- **Ergonomic gain is mostly neutralized.** The `windows` crate's advantages
  (typed `HINTERNET`, `Result`-returning wrappers, `PCWSTR`/`w!` strings) buy
  little here: we build our own RAII handle wrapper regardless (the `windows`
  handle does not auto-close), the status callback is `unsafe extern "system"`
  in both, and the `dwContext` raw-pointer bridge is identical and raw in both.
  All of it is already contained under `src/ffi/`, so the safe-surface benefit is
  small. The only real convenience — UTF-16 string handling — is wrapped once
  internally.

This decision would flip only if `fetch_winhttp` later moved to (or were shared
from) ox-sdk2, where `windows` is already a dependency.

## Testing strategy

- Unit tests for pure logic (option translation, error/label classification,
  URL/verb formatting) need no network and no WinHTTP.
- The pipeline-level fake path already exists in `fetch`
  (`test-util` → `FakeHandler`), so higher layers are tested without this crate.
- End-to-end WinHTTP tests are Windows-only and gated (`#[cfg(windows)]`,
  `#[cfg_attr(miri, ignore)]`), pointed at a local `wiremock`-style server.
- Follow the `AGENTS.md` naming rule: **no** `setup`/`install`/`update`/`patch`
  substrings in test/example/bench binary names (Windows UAC elevation heuristic
  breaks the harness).
