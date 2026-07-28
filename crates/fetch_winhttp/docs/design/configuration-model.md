<!-- Copyright (c) Microsoft Corporation. Licensed under the MIT License. -->

# Configuration model: knob buckets

This document defines the configuration architecture `fetch_winhttp` is designed
against. It resolves the root cause of the WinHTTP "mismatch" problem: today a
single, hyper-shaped `TransportOptions` (and `TlsOptions`) is handed to *every*
transport, so a transport with a different model (WinHTTP) is forced to Honor /
Approximate / Reject knobs that don't fit it.

The fix is to classify every configuration knob into one of four buckets by two
questions: *is it portable across transports?* and *who is expected to set it —
the library wrapping `fetch`, or the consumer choosing the transport?* The
buckets drive where each knob is surfaced. [`builder-architecture.md`](builder-architecture.md)
defines the type-level mechanics (concrete builder, object-safe `Transport`
trait, `PortableOptions`); this doc defines the classification and why each knob
lands where it does.

## Why buckets, not a single options struct

A single `TransportOptions` handed to every transport forces WinHTTP to pretend to
honor hyper-shaped knobs (connection pool internals, keep-alive PING timings,
HTTP/2 stream tuning, userspace TLS backend selection). Splitting by portability
means WinHTTP is only ever handed knobs it can actually honor, and its own native
strengths get a first-class home instead of being wedged into a hyper-shaped
field.

## The four buckets

| Bucket | Portable? | Set where | Set by | Transport role |
| --- | --- | --- | --- | --- |
| **A — Pipeline** | Yes | `HttpClientBuilder` methods | library / consumer | none (enforced above the transport) |
| **B — Required portable** | Yes | `HttpClientBuilder` → `PortableOptions` | library / consumer | must honor, or fail fast |
| **B2 — Optional portable** | Yes | `HttpClientBuilder` → `PortableOptions` | library / consumer | capability-gated: honor if able; security → fail fast, perf → documented behavior |
| **C — Transport-specific** | No | the transport's **own** builder | consumer only | honored natively by that transport |

Buckets A + B + B2 are the **transparency contract**: they are set the same way
and mean the same thing regardless of transport, and a library can rely on them
across the transport a consumer later picks. Bucket C is explicitly
transport-specific — reaching for it is, correctly, writing transport-specific
code, and it is the consumer's job (the consumer chose the transport).

## Bucket-by-bucket classification

### A — pipeline-owned

Consumed by pipeline layers above the transport leaf, so no transport sees them:

- scheme policy (`request_filter`) — enforced in `Dispatch::validate`
- resilience: retry / hedging / circuit-breaking / timeout (`seatbelt`)
- telemetry / metrics / logging
- base URI / router; redaction
- response-body buffering and idle limits (`HttpBodyOptions`)
- multi-pool `(count, selection)` — runs *N* transport handlers and load-balances;
  a `fetch` concept distinct from a transport's internal connection pool

### B — required portable

Every transport must honor these (or fail fast). Pushed to the transport via
`PortableOptions`:

- `connect_timeout`
- HTTP-version preference (`supported_http_versions`, may include HTTP/3)
- mTLS `client_identity` — honor, or fail fast if unsupported
- `max_connections_per_host`
- streaming request/response bodies; per-request `Extensions` passthrough
- cancellation on future-drop

### B2 — optional portable (capability-gated)

Portable knobs a library may legitimately need, but not every transport honors
identically. Each carries an explicit contract:

- **certificate pinning / validation policy** — security semantics: honor or
  **fail fast**. Portable on both transports (hyper custom verifier; WinHTTP via
  `WINHTTP_OPTION_SERVER_CERT_CONTEXT` in the status callback).
- **connection idle timeout** — perf semantics. WinHTTP has a built-in ~60s idle
  scavenger and native levers to shorten (`WINHTTP_OPTION_EXPIRE_CONNECTION`) or
  prolong an HTTP/2 connection (`WINHTTP_OPTION_HTTP2_KEEPALIVE`); see
  [`winhttp-capabilities.md`](winhttp-capabilities.md).
- **connection max-lifetime** — perf semantics. Native on WinHTTP via
  `WINHTTP_OPTION_EXPIRE_CONNECTION`; hyper via connection poisoning.
- **coarse keep-alive** (on/off + idle-triggered health check) — perf semantics;
  the idle case maps to WinHTTP `WINHTTP_OPTION_HTTP2_KEEPALIVE`.

### C — transport-specific (consumer-only)

Set on the transport's own builder, before erasure:

- hyper: connection-pool poisoning internals, keep-alive PING probe *timings*,
  HTTP/2 stream tuning (`initial_max_send_streams`, `adaptive_window`), userspace
  TLS backend selection + custom verifier beyond the portable pinning hook.
- WinHTTP: proxy / WPAD, integrated Windows auth, per-server tuning beyond the
  portable cap, SChannel-specific options.

## Non-negotiable invariants (not knobs)

Some behaviors are intentionally *not* configurable, because making them optional
would be a footgun:

- **Certificate revocation checking is always on.** Every transport checks
  revocation unconditionally. On WinHTTP this means the transport must enable
  `WINHTTP_ENABLE_SSL_REVOCATION` (SChannel does not hard-fail on revocation by
  default under WinHTTP); on hyper the platform verifier's revocation path must
  stay enabled. There is no "disable revocation" knob in any bucket.

## Where the current option types go

- `fetch_options::TransportOptions` shrinks toward `PortableOptions` (Bucket B/B2
  plus the `extra` escape hatch). `request_filter` stays a builder method but is a
  Bucket-A pipeline concern, not pushed to the transport.
- `ConnectionPoolOptions` decomposes: `max_connections` → B; idle-timeout and
  max-lifetime → B2; poisoning internals and multi-pool selection → A (multi-pool)
  or C (hyper poisoning).
- `ConnectionKeepAlive` and `Http2Options` (stream tuning) → C on `fetch_hyper`,
  with the coarse keep-alive case surfaced as a B2 knob.
- `fetch_tls::TlsOptions`: `client_identity` and version preference are B; the
  pinning/validation policy is B2; backend selection + custom verifier are C.

## Enforcement → see `builder-architecture.md`

The buckets are surfaced through a **concrete** (non-generic) `HttpClientBuilder`
and `HttpClient`, with the transport behind an object-safe `Transport` trait.
Bucket A/B/B2 knobs are methods on the concrete builder (so a library can accept a
`HttpClientBuilder` without generics and layer its config); Bucket C knobs live on
the transport's own builder (so the consumer configures them before handing the
builder to the library). Mocking is just another `Transport`. See
[`builder-architecture.md`](builder-architecture.md) for the type mechanics, the
library/consumer/mock composition, and backwards compatibility.
