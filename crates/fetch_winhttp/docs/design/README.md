<!-- Copyright (c) Microsoft Corporation. Licensed under the MIT License. -->

# `fetch_winhttp` design

This folder captures the design for a WinHTTP-backed transport for the
[`fetch`](../../../fetch) HTTP client. It is written before implementation so
the architecture, the integration contract with `fetch`, and the known
configuration-mapping tensions are agreed up front.

## Goal

Let a `fetch::HttpClient` issue requests through the Windows **WinHTTP** stack
instead of the default `fetch_hyper` (hyper + rustls/native-tls) transport,
**without changing the client-facing API**. A caller opts in at construction
time; every request they make afterwards — and the entire pipeline above the
transport (buffering, metrics, logging, retry, hedging, circuit breaking) —
behaves identically.

## Why WinHTTP

WinHTTP is the Windows-native HTTP client stack. Choosing it buys:

- **HTTP/3 (QUIC) support.** WinHTTP negotiates HTTP/3 on recent Windows
  (Windows 11 / Server 2022+, via `WINHTTP_PROTOCOL_FLAG_HTTP3`), which the
  default `fetch_hyper` transport does **not** offer — `hyper` has no stable
  HTTP/3 support, so today `fetch` tops out at HTTP/2. This is the single
  capability WinHTTP adds that no existing `fetch` transport can match.
- **OS-managed TLS (`SChannel`)** and the Windows certificate stores /
  revocation infrastructure, rather than a bundled rustls/native-tls stack.
  This matters for enterprise/compliance environments (FIPS, group-policy trust,
  auto-enrolled roots) and for staying aligned with the OS security posture.
- **Native proxy support** (WPAD, per-user proxy config, authenticated proxies)
  and integrated Windows authentication (Negotiate/NTLM) that WinHTTP handles
  internally.
- **One fewer bundled TLS/crypto dependency** on Windows-only deployments.

## The one architectural fact that makes this clean

`fetch`'s pipeline is transport-agnostic. The leaf of every pipeline is a
*transport handler*: any `Service<HttpRequest, Result<HttpResponse>>` (the
`RequestHandler` alias from `http_extensions`). Everything else is layered on
top and does not care how bytes reach the wire. `fetch` already exposes a
first-class extension point for supplying your own leaf —
`fetch::custom::create_builder(...)` — which both the built-in Tokio transport
and the `fetch_m365` runtime constructors use.

So WinHTTP support is *additive*: a new crate that produces a `RequestHandler`,
plus thin factory wiring. No pipeline surgery.

## The one fact that makes it non-trivial

WinHTTP is a **complete** HTTP stack: DNS, connection pooling, proxy, auth, TLS,
and HTTP framing all happen *inside* WinHTTP. It is therefore **not** a
`fetch_hyper`-style connector (which supplies raw byte streams and lets
hyper+rustls do framing and TLS). WinHTTP replaces the whole `HyperTransport`,
and several `fetch` configuration knobs that assume the hyper/rustls model do
not map onto it one-to-one.

Rather than force WinHTTP to Honor/Approximate/Reject a hyper-shaped options
struct, the design classifies every knob into buckets by portability and owner
(pipeline / required-portable / optional-portable / transport-specific), and
surfaces them through a **concrete** (non-generic) `HttpClientBuilder` so a
library wrapping `fetch` can accept and configure a builder without viral
generics, while the consumer picks the transport (or a mock). This is the
substance of the design — see [`configuration-model.md`](configuration-model.md)
and [`builder-architecture.md`](builder-architecture.md).

## Documents

| Document | Contents |
| --- | --- |
| [`architecture.md`](architecture.md) | Where the crate sits, the public API shape, how it plugs into `fetch` and `fetch_m365`, crate layout, planned dependencies, `unsafe`/FFI policy. |
| [`configuration-model.md`](configuration-model.md) | The knob-bucket classification (A pipeline / B required-portable / B2 optional-portable / C transport-specific) that resolves the mismatch at its root, plus the always-on invariants. |
| [`builder-architecture.md`](builder-architecture.md) | Why both `HttpClient` and `HttpClientBuilder` stay concrete (the library-accepts-a-builder + mock requirement), the object-safe `Transport` seam, `PortableOptions`, and the hyper backwards-compatibility plan. |
| [`winhttp-capabilities.md`](winhttp-capabilities.md) | Factual WinHTTP reference: handle model, session-scoped pooling, the ~60s idle scavenger, keep-alive taxonomy, and the `WinHttpSetOption` inventory the design relies on. |
| [`async-bridge.md`](async-bridge.md) | How WinHTTP's asynchronous completion callbacks are turned into runtime-agnostic Rust futures; handle lifecycle; cancellation; streaming bodies. |
| [`configuration-mapping.md`](configuration-mapping.md) | The portable-bucket mapping onto WinHTTP mechanisms, and which hyper-shaped knobs are transport-specific (and why). |

## Non-goals (for the initial design)

- Replacing `fetch_hyper` as the default transport. Hyper stays the
  cross-platform default; WinHTTP is opt-in.
- Non-Windows support. The crate is `#[cfg(windows)]` end to end.
- Exposing every WinHTTP capability. Proxy/auth integration is sketched but
  phased; the first cut targets correctness parity for direct HTTPS requests.
