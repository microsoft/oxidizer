# `fetch_winhttp` design

Status: design, pre-implementation. This document describes the architecture, behavior, and design tenets of the `fetch_winhttp` crate. The implementation strategy - threading, FFI ownership, pooling, body-streaming mechanics, and the test plan - is documented separately in [implementation.md](implementation.md). The crate currently ships only these design documents and a placeholder `lib.rs`.

## 1. Purpose and scope

`fetch_winhttp` is a Windows-only custom transport for the [`fetch`] HTTP
client. It services `fetch` requests by driving the operating system's [WinHTTP]
client API in asynchronous WinHTTP I/O mode, as an alternative to the bundled
`fetch_hyper` (hyper + rustls/native-tls) transport.

Why a WinHTTP transport:

- **OS-managed TLS/trust.** WinHTTP terminates TLS through Schannel and uses the
  Windows certificate stores and system trust policy. Applications that must
  honor enterprise trust configuration or CTLs get that without bundling a userland
  TLS stack. (Client certificates are a Schannel capability but are not exposed in
  v1; see §4.1.)
- **OS-managed protocol stack.** HTTP/1.1, HTTP/2 and HTTP/3 negotiation,
  connection pooling, keep-alive, proxy discovery and automatic gzip/deflate
  decompression are handled by the OS.
- **Smaller dependency surface.** No rustls/aws-lc-rs/native-tls/hyper on the
  request path.

Out of scope: any non-Windows platform (the crate is `#[cfg(windows)]` in its
entirety); WebSocket upgrades; proxy auto-config scripting beyond what WinHTTP
does natively.

### 1.1 Constructing a client

A caller builds a WinHTTP-backed client the same way as the bundled Tokio transport,
except the constructors arrive through an extension trait this crate implements on
`fetch::HttpClient` (imported into scope):

```rust,ignore
use fetch::HttpClient;
use fetch_winhttp::{HttpClientWinHttpExt, WinHttpDeps, WinHttpOptions, WinHttpTlsConfig};

// Clock, memory pool, and telemetry sink come from the application's environment.
// TLS and WinHTTP-specific user configuration default when omitted.
let deps = WinHttpDeps::builder()
    .clock(clock)
    .global_pool(global_pool)
    .sink(sink)
    .tls(WinHttpTlsConfig::builder()
        .accept_invalid_certs(true)                 // Schannel knobs, §4
        .build())
    .options(WinHttpOptions::builder()
        .resolve_timeout(Duration::from_secs(10))   // optional native DNS-only deadline, §6
        .build())
    .build();

let client = HttpClient::builder_winhttp(deps)
    .build();                    // a `fetch::HttpClientBuilder`, so the pipeline can be tuned first
```

The result is an ordinary `fetch` `HttpClient`; no other caller code changes.
`WinHttpDeps` carries the mandatory environment dependencies needed by this transport:
the timer-capable `tick::Clock`, `bytesbuf::mem::GlobalPool`, and `observed::Sink`.
These values cannot be invented by the crate and therefore have no defaults. Its TLS
and WinHTTP option fields are user configuration and do default. `WinHttpDeps` and its
component config types are `#[non_exhaustive]` and constructed through builders so new
fields can be added compatibly:

```rust,ignore
/// WinHTTP-specific dependencies. Construct with [`WinHttpDeps::builder`].
#[derive(thread_aware::ThreadAware)]
#[non_exhaustive]
pub struct WinHttpDeps { /* clock, global pool, sink, TLS, options - private */ }

impl WinHttpDeps {
    /// Starts building a `WinHttpDeps`.
    ///
    /// `clock`, `global_pool`, and `sink` are mandatory. `build()` panics with an
    /// actionable programming-error message if any is missing.
    pub fn builder() -> WinHttpDepsBuilder;
}

/// Adds WinHTTP-transport constructors to `fetch::HttpClient`.
pub trait HttpClientWinHttpExt {
    /// Returns a builder for an `HttpClient` on the WinHTTP transport.
    fn builder_winhttp(deps: impl Into<WinHttpDeps>) -> HttpClientBuilder;
}
```

`WinHttpTlsConfig` (§4) and `WinHttpOptions` (§3, §5, §6) follow the same
builder + `#[non_exhaustive]` pattern.

The crate is pre-1.0 and the extension-trait constructor shape may change as the
`fetch` custom-transport API is stabilized. The behavioral contract in this document
guides v1 implementation; it does not promise that this provisional construction API is
already stable.

### 1.2 TLS is configured on the transport, not through `fetch`'s `TlsOptions`

`fetch`'s generic `TlsOptions`/`TlsBackend` carries rustls/native-tls material
(crypto providers, verifiers, client-cert resolvers) that is meaningless to
Schannel. WinHTTP does TLS itself and accepts only a small set of knobs, so
`fetch_winhttp` therefore ignores `fetch`'s TLS configuration entirely and takes its
own `WinHttpTlsConfig` instead (§4). Different transports inherently support different TLS
configuration models, so trying to configure TLS uniformly at the transport-abstract
`fetch` level is over-abstraction on `fetch`'s part; see the fetch API stabilization
feedback (../../fetch/docs/stabilization.md).

## 2. Connection management

WinHTTP owns connection establishment, pooling, keep-alive, and reuse; the transport
does not open, bind, or pool sockets itself (the mechanics are in implementation.md §9).
This chapter states what that means for the caller: the guarantees the transport
provides, which `fetch` connection options it honors and to what fidelity (§2.1), and
which it cannot honor (§2.2).

**Pool isolation is guaranteed.** Each independently built `HttpClient` gets its own
isolated set of connection pools - one per materialized (core × pool slot) transport
instance. Two independently built clients never reuse each other's connections, even in
the same process. This is a security boundary: a strict client and one built with
`accept_invalid_certs` (§4) must not share a pooled TLS connection. Cloned `HttpClient`
values share the original client's pool set; within each pool, connections are reused
normally.

### 2.1 Mapping generic transport options onto WinHTTP

`fetch::custom::CustomContext` supplies generic `TransportOptions` and `TlsOptions`.
WinHTTP's controls do not map one-to-one, so some values are exact, some approximate,
and some ignored:

| `fetch` option | WinHTTP mechanism | Fidelity |
|----------------|-------------------|----------|
| `connect_timeout` | `tick::Clock` race around connection establishment | Exact total deadline; no native connect timer. |
| `request_filter` | request URI validation plus `WINHTTP_FLAG_SECURE` | Exact. |
| `supported_http_versions` | WinHTTP enable/required protocol options | Exact for HTTP/1.1, HTTP/2, and HTTP/3; other versions are rejected. |
| `multiple_pools` | `fetch` materializes a separate transport/session for every pool slot | Exact structural isolation; the selection strategy remains owned by `fetch`. |
| `max_connections = usize::MAX` (default) | nothing to do | Exact. |
| finite `max_connections` | ignored | Not honored: `fetch` limits idle retained connections, whereas WinHTTP's available option limits all physical connections and could throttle active HTTP/1.1 requests. |
| `connection_idle_timeout` | WinHTTP's own idle keep-alive management; `PurgeKeepAlives` to force-clear | Not honored: WinHTTP exposes no idle-TTL knob, so the configured value has no effect and WinHTTP applies its own default. |
| `connection_lifetime = Unlimited` (default) | nothing to do | Exact. |
| `connection_lifetime = Fixed(_)` / `PerConnection(_)` | not honored in v1 (see §2.2) | Not honored (see §2.2). |
| `ConnectionKeepAlive::Disabled` (default) | leave keep-alive at WinHTTP defaults | n/a |
| `ConnectionKeepAlive::ActiveConnections { interval, timeout }` | HTTP/2/3 keep-alive interval, floored to 5000 ms | Approximate: WinHTTP also probes idle connections and manages its own response timeout; HTTP/1.1 has no equivalent probe. |
| `ConnectionKeepAlive::ActiveAndIdleConnections { interval, timeout }` | same as `ActiveConnections` | Approximate: WinHTTP does not distinguish the two modes and ignores the generic `timeout`. |
| `Http2Options::initial_max_send_streams` | ignored | Not honored: WinHTTP owns HTTP/2 stream concurrency. |
| `Http2Options::adaptive_window` | ignored | Not honored: WinHTTP owns HTTP/2 flow control. |
| `TransportOptions::extra` | ignored | No v1 WinHTTP extension types are defined in the generic extension map. |
| generic TLS `supported_http_versions` | ignored | Protocol selection comes from `TransportOptions::supported_http_versions`. |
| generic TLS `client_identity` | ignored | Client certificates are out of scope (§4.1). |
| generic TLS automatic/backend selection | ignored | Schannel/WinHTTP is always the backend. |
| preconfigured rustls/native-tls backend | ignored | Those backend objects cannot configure WinHTTP. |
| rustls crypto provider or certificate verifier | ignored | Schannel owns cryptography and certificate verification. |
| rustls client-certificate resolver | ignored | Client certificates are out of scope (§4.1). |

`ConnectionInfo` (age, `is_expired`, poisoning) that `fetch_hyper` attaches to
responses cannot be reproduced: WinHTTP hides individual connections, so per
connection age is not observable and no per-connection identity is exposed.
(`WINHTTP_OPTION_EXPIRE_CONNECTION` can blindly retire the connection a given
request rode, but without age/identity visibility it cannot drive the
age-conditional poisoning `ConnectionInfo` models; see §2.2.) A response from
this transport carries no `ConnectionInfo`. Every approximation or omission above
is a property of WinHTTP's opaque pool, not of this transport.

### 2.2 Connection lifetime (bounded connection age)

`fetch`'s `connection_lifetime` option asks the client to stop reusing a
connection once it reaches a maximum age (`Fixed(d)`: every connection expires
after `d`; `PerConnection(f)`: a per-connection age drawn from `f`). The intent is
to bound how long any single TCP/TLS connection stays in service so long-lived
clients periodically re-establish connections (load-balancer rebalancing, cert
rotation, routing changes).

WinHTTP does not expose individual connections, so no available mechanism bounds
connection age faithfully:

| Mechanism | Effect | Why insufficient |
|-----------|--------|------------------|
| `WINHTTP_DISABLE_KEEP_ALIVE` (per request) | Closes the request's connection afterward | Caps reuse but cannot express "expire after `d`"; disables pooling wholesale |
| `WINHTTP_OPTION_EXPIRE_CONNECTION` (per request) | Stops WinHTTP returning that physical connection to the keep-alive pool | Closest analogue to per-connection poison, but WinHTTP exposes no connection identity or age, so we cannot target only over-age connections - only every request (equivalent to the row above) or none |
| Whole-session recycling | Open a fresh session, steer new requests to it, drain and close the old one | Bounds age only pool-wide, not per connection, and needs a drain latch, age timer, and atomic session swap for an approximate result |

Because none is faithful, **v1 does not implement `connection_lifetime` for
`Fixed`/`PerConnection`; it never recycles sessions during the transport's lifetime.**
(`WINHTTP_OPTION_EXPIRE_CONNECTION` remains a candidate for a different feature:
error-driven poisoning of a connection after a protocol failure.)

Unsupported generic connection options are ignored without runtime diagnostics. Their
fidelity is documented here so callers can select transport-specific configuration
knowingly. A future version may add a proper connection-lifetime mechanism (most likely
whole-session recycling gated behind an explicit opt-in, given its cost and coarse
granularity). That this option arrives from the `fetch` layer at all, rather than being
configured on the transport that owns the connections, is noted as `fetch` API feedback
in the fetch API stabilization feedback (../../fetch/docs/stabilization.md).

### 2.3 Proxy discovery

The session is opened with `WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY`. WinHTTP applies the
current Windows proxy policy, including automatic discovery and PAC handling available
through that mode. v1 exposes no proxy configuration and has no `DEFAULT_PROXY` or direct
connection fallback: silently bypassing OS proxy policy would violate the purpose of
using the Windows networking stack.

The transport targets modern Windows versions where automatic proxy mode and every other
WinHTTP option required by this design are available. It performs no old-Windows
compatibility probing or degradation. A required session option that cannot be applied
leaves that materialized transport in its permanent initialization-failure state; a
required request option that cannot be applied fails that request.

## 3. HTTP protocol negotiation

The transport supports HTTP/1.1, HTTP/2, and HTTP/3, all as first-class modes. Which
versions a request may use comes from `fetch`'s `TransportOptions.supported_http_versions`:

- The listed versions are the ones allowed. An empty list means "no preference" and uses
  `fetch`'s default (HTTP/1.1 and HTTP/2).
- Listing only versions newer than HTTP/1.1 (for example HTTP/2 and/or HTTP/3 without
  HTTP/1.1) disables the HTTP/1.1 fallback: if none of the required protocols can be
  negotiated the request fails rather than downgrading.
- A version the transport cannot speak (`HTTP/0.9`, `HTTP/1.0`) is rejected at request
  construction with an `invalid_request` error, never silently dropped.

Negotiation, including ALPN, is performed by the OS during the TLS handshake; the
transport does not negotiate manually. The version actually negotiated is reported on the
returned `HttpResponse`, so telemetry reflects what was negotiated rather than what was
requested. The transport assumes the required modern WinHTTP protocol options exist and
does not probe feature availability or provide an older-version fallback. Failure to
apply a required protocol option fails the request. (How the version set is expressed to
WinHTTP is implementation.md §10.1.)

## 4. TLS

TLS is handled by the OS through Schannel (§1); the transport ships no userland root
bundle and configures only a small set of `WinHttpTlsConfig` knobs (§1.2):

- **`https` selection.** `https://` targets use TLS. `http://` is issued only when the
  client is built with `insecure_allow_http()` and the request filter admits it -
  identical policy to the other transports.
- **Insecure mode.** `accept_invalid_certs` / `accept_invalid_hostnames` disable
  certificate and host-name validation respectively. This is the insecure mode called out
  in the requirements; it is opt-in and documented as dangerous.
- **Server certificate inspection / pinning.** Beyond accept/reject, not offered in v1.

(How these knobs reach Schannel is implementation.md §10.2.)

### 4.1 Client certificates (mTLS) are out of scope for v1

`fetch` does not require a transport to support client certificates: its mTLS surface
(`fetch::tls::ClientIdentity`) travels inside the generic `TlsOptions` that `fetch_winhttp`
deliberately ignores (§1.2), and a transport that offers no client identity is a
conforming `fetch` transport. Client certificates are an uncommon feature the large
majority of callers never use, and supporting them is a self-contained chunk of future
work with its own lifetime and ownership concerns.

v1 therefore does not implement client certificates; `WinHttpTlsConfig` exposes no
client-identity field. A later iteration can add a WinHTTP-specific client-identity type
if a concrete need appears.

## 5. WinHTTP-managed HTTP behavior

The OS handles several HTTP behaviors internally. The transport configures each so it
behaves consistently with the rest of `fetch`:

- **Automatic decompression (always on).** The transport advertises
  `Accept-Encoding: gzip, deflate`; gzip/deflate responses are transparently decoded
  before the body is returned, with `Content-Encoding`/`Content-Length` stripped, so
  callers always see a decoded body. `fetch` itself has no content decoding, so there is
  no double-decode risk. No opt-out is exposed in v1, since it would only hand callers an
  encoded body nothing downstream can decode.
- **Brotli/zstd.** Not decoded (the OS does not support them); such responses arrive
  still-encoded with `Content-Encoding` intact and pass through verbatim.
- **Request-body compression.** Not performed automatically; a caller that pre-encodes its
  body and sets `Content-Encoding` has it sent as-is.
- **Streaming request bodies.** Known- and unknown-length bodies are supported with
  HTTP/1.1, HTTP/2, and HTTP/3. The transport streams every data frame through
  `WinHttpWriteData`, waits for each write completion, and calls
  `WinHttpReceiveResponse` only after the request body reaches end-of-stream. WinHTTP is
  responsible for the protocol-appropriate framing of an unknown total length.
- **Trailers.** Response trailers exposed by WinHTTP are returned as `HttpBody` trailer
  frames rather than discarded. WinHTTP has no request-trailer submission API, so an
  outgoing trailer frame fails the request rather than being silently dropped.
- **Redirects are not followed.** Like `fetch_hyper` (and unlike WinHTTP's own default),
  3xx responses are surfaced to the caller unchanged rather than followed, with no knob
  to re-enable automatic redirects.
- **Cookies and automatic authentication are disabled.** The transport keeps no cookie
  store and does not answer `WWW-Authenticate`/407 challenges; `Set-Cookie`/`Cookie` and
  challenge responses pass through as plain data for the caller to manage. The transport
  is thus stateless between requests.

(The specific OS options behind each behavior are implementation.md §10.3.)

## 6. Timeouts and time

Timeouts are enforced by `fetch`-layer futures driven by the caller-supplied `tick::Clock`
wherever the required interval is observable outside WinHTTP. Native WinHTTP timers are
configured as unlimited by default and are used only for a deadline that cannot be
expressed outside WinHTTP.

### 6.1 Which timeouts the transport honors

- **Connect timeout** (`TransportOptions.connect_timeout`, default 30 s): honored by this
  transport as a *total* deadline on connection establishment (§6.2). `fetch` core models
  this option but leaves each transport to enforce it. The WinHTTP transport races the
  observable connection-establishment phase against `tick::Clock`; it does not use
  WinHTTP's per-address native connect timer.
- **Response timeout** (`http_extensions::ResponseTimeout`, read per-request from the
  request extensions): a *total* deadline over connection setup, sending the request, and
  receiving the response headers. `fetch` enforces this above the transport (the same way
  `fetch_hyper` relies on it), and it surfaces as `HttpError::timeout`. The transport does
  not remap it onto any native WinHTTP timer.
- **Body idle timeout** (`http_extensions::BodyTimeout`, read per-request from the request
  extensions): the maximum idle gap between response body frames, reset on progress. The
  transport passes this value to the supplied `HttpBodyBuilder`, which merges it with
  client-level response-body defaults and applies the `fetch` body timeout wrapper. No
  native WinHTTP receive timer is required.
- **Seatbelt request timeout**: enforced above the transport; the transport is not
  involved.
- **Send timeout**: not a distinct concept. Sending the request and waiting for the
  response headers (without touching the body) is exactly the span `ResponseTimeout`
  already governs, after which `BodyTimeout` takes over; there is no separate send
  deadline to honor.
- **Resolve timeout**: `fetch` has no concept for a standalone DNS-resolution deadline
  and WinHTTP does not expose the DNS stage as a separately awaitable operation. It is
  therefore the one native timeout exposed through transport-specific
  `WinHttpOptions::resolve_timeout`. It defaults to unlimited; when explicitly configured,
  it is applied through `WinHttpSetTimeouts`.

When any `fetch`-layer timeout drops the request future or response body, the transport
closes the WinHTTP handle and tears the request down (implementation.md §4).

### 6.2 The outer connect timeout

WinHTTP's native connect timer bounds only a single per-address connection attempt.
A multi-homed host can therefore exceed `TransportOptions.connect_timeout` while trying
several addresses. `fetch` callers expect a total deadline, so the transport enforces it
itself with `tick::Clock` and leaves the native connect timer unlimited (how it does so
is implementation.md §4.6).

The deadline spans connection establishment: name resolution, TCP/TLS connect,
proxy discovery, and sending the request line and headers. The request body is
streamed afterward, so it lies outside this connect deadline. It remains inside the
per-request `ResponseTimeout`, which continues through the complete upload and response
headers (§6.1).

One consequence: the deadline can fire after the headers reached the server, so a
bodyless non-idempotent request may already be in processing when it trips.
Whether that request is safe to retry is `seatbelt`'s concern, not the
transport's; the transport only reports the timeout.

## 7. Error handling model

`fetch` transports return `Result<HttpResponse, HttpError>`. `HttpError`
(`http_extensions`) carries a source error, an `ohno::ErrorLabel`, and a
`recoverable::RecoveryInfo`, mirroring `fetch_hyper`:

- **Error surface.** Two Win32 error sources: the last-error from a failing
  synchronous call, and `dwError` from `WINHTTP_ASYNC_RESULT` on a
  `REQUEST_ERROR` callback. `SECURE_FAILURE` may additionally supply a bitmask of
  certificate problems, which is attached when available as best-effort diagnostics.
- **Mapping.** A Win32/`WINHTTP_*` code is turned into
  `HttpError::other(WinHttpError { code, .. }, recovery, label)`.
- **Labels** (mirroring `fetch`'s own error labels):

  | Condition | `ErrorLabel` |
  |-----------|--------------|
  | `ERROR_WINHTTP_CANNOT_CONNECT`, `NAME_NOT_RESOLVED` | `connect` |
  | `ERROR_WINHTTP_TIMEOUT` | `timeout` |
  | `ERROR_WINHTTP_SECURE_FAILURE` | `tls` |
  | `ERROR_WINHTTP_OPERATION_CANCELLED` | `abandoned` |
  | send/receive/protocol failures | `request_winhttp` |

### 7.1 Recoverability rationale

`recoverable::RecoveryInfo` feeds `seatbelt`'s retry and breaker layers above the
transport. The division is not arbitrary; the rule is: an error is retryable iff
retrying the identical request (on a fresh connection) could plausibly succeed
without the caller changing anything. Idempotency and retry budgets are
`seatbelt`'s concern, not ours; we only classify whether the failure is transient
transport noise or a deterministic condition.

- **Retryable** (transient transport/connection faults): connection reset or
  closed mid-flight, `NAME_NOT_RESOLVED` (DNS can be flaky), `CANNOT_CONNECT`
  (transient server/pool state), `TIMEOUT` and `CONNECTION_ERROR` (transient
  load). Re-issuing may land on a healthy connection.
- **Never** (deterministic failures): TLS/certificate validation failures (given
  a fixed trust configuration, a retry yields the same verdict) and
  `OPERATION_CANCELLED` (the caller initiated teardown; retrying would contradict
  intent). Malformed-response/protocol violations that indicate a stable server
  or configuration problem are also non-retryable.

HTTP status codes (4xx/5xx) never enter this mapping: they are successful
transport outcomes carrying an error status, surfaced as `Ok(HttpResponse)`, and
any retry policy on them lives in `seatbelt` above the transport. Automatic
decompression handled by WinHTTP never surfaces as a transport error; only genuine
wire/OS failures do.

[`RequestHandler`]: https://github.com/microsoft/oxidizer/tree/main/crates/http_extensions
[WinHTTP]: https://learn.microsoft.com/en-us/windows/win32/winhttp/using-winhttp
