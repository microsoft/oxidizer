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
  connection pooling, keep-alive, and proxy discovery are handled by the OS.
  Response decompression remains in `fetch` so every transport has identical
  content semantics.
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

// Defaults:
let client = HttpClient::new_winhttp();

// Configured. Every WinHTTP config type is built through a builder, never a struct
// literal, so all of them are `#[non_exhaustive]` and can gain knobs in later versions
// without a breaking change:
let deps = WinHttpDeps::builder()
    .tls(WinHttpTlsConfig::builder()
        .accept_invalid_certs(true)                 // Schannel knobs, §4
        .build())
    .options(WinHttpOptions::builder()
        .resolve_timeout(Duration::from_secs(10))   // transport-specific tuning, §3, §5, §6
        .build())
    .sink(observed::Sink::noop())                   // telemetry sink; detailed in v1.1
    .build();

let client = HttpClient::builder_winhttp(deps)
    .build();                    // a `fetch::HttpClientBuilder`, so the pipeline can be tuned first
```

The result is an ordinary `fetch` `HttpClient`; no other caller code changes.
`WinHttpDeps` carries only WinHTTP-specific configuration - the clock, memory pool, and
telemetry meter are supplied by `fetch` itself and are not configured here. It and its
component config types are `#[non_exhaustive]` and constructed through builders so new
fields can be added compatibly:

```rust,ignore
/// WinHTTP-specific dependencies. Construct with [`WinHttpDeps::builder`].
#[derive(thread_aware::ThreadAware)]
#[non_exhaustive]
pub struct WinHttpDeps { /* tls, options, sink - private; set via the builder */ }

impl WinHttpDeps {
    /// Starts building a `WinHttpDeps`. `tls`/`options`/`sink` default when unset.
    pub fn builder() -> WinHttpDepsBuilder;
}

/// Adds WinHTTP-transport constructors to `fetch::HttpClient`.
pub trait HttpClientWinHttpExt {
    /// Returns a builder for an `HttpClient` on the WinHTTP transport.
    fn builder_winhttp(deps: impl Into<WinHttpDeps>) -> HttpClientBuilder;
    /// Builds an `HttpClient` on the WinHTTP transport with default deps.
    fn new_winhttp() -> HttpClient;
}
```

`WinHttpTlsConfig` (§4) and `WinHttpOptions` (§3, §5, §6) follow the same
builder + `#[non_exhaustive]` pattern.

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

**Pool isolation is guaranteed.** Each `HttpClient` this transport builds gets its own
connection pool; two independently built clients never reuse each other's connections,
even in the same process. This is a security boundary: a strict client and one built
with `accept_invalid_certs` (§4) must not share a pooled TLS connection. Within a single
client, connections are reused normally.

### 2.1 Mapping `fetch` connection-pool options onto WinHTTP

`fetch_options::ConnectionPoolOptions` (reached via `TransportOptions`) exposes
`max_connections`, `connection_idle_timeout`, and `connection_lifetime`, plus
`ConnectionKeepAlive`. WinHTTP's controls do not map one-to-one, so some options are
honored only approximately and others cannot be honored at all:

| `fetch` option | WinHTTP mechanism | Fidelity |
|----------------|-------------------|----------|
| `max_connections` (per pool) | `WINHTTP_OPTION_MAX_CONNS_PER_SERVER` | Approximate: the limit is applied, but WinHTTP enforces it per authority whereas `fetch` counts per pool, so a multi-host pool's effective cap differs. |
| `connection_idle_timeout` | WinHTTP's own idle keep-alive management; `PurgeKeepAlives` to force-clear | Not honored: WinHTTP exposes no idle-TTL knob, so the configured value has no effect and WinHTTP applies its own default. |
| `connection_lifetime = Unlimited` (default) | nothing to do | Exact. |
| `connection_lifetime = Fixed(_)` / `PerConnection(_)` | not honored in v1 (see §2.2) | Not honored (see §2.2). |
| `ConnectionKeepAlive::ActiveConnections{..}` | `WINHTTP_OPTION_HTTP2_KEEPALIVE` / `WINHTTP_OPTION_HTTP3_KEEPALIVE` (floor 5000 ms) | Approximate for h2/h3; HTTP/1.1 keep-alive is automatic. |
| `ConnectionKeepAlive::Disabled` (default) | leave keep-alive at WinHTTP defaults | n/a |

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

Rather than silently ignore a `Fixed`/`PerConnection` setting - which would let a
caller believe a bounded-connection-age guarantee is in force when it is not - the
transport warns at build time per the general unhonorable-option policy
(implementation.md §11). A
future version may add a proper mechanism (most likely whole-session recycling gated
behind an explicit opt-in, given its cost and coarse granularity). That this option
arrives from the `fetch` layer at all, rather than being configured on the transport
that owns the connections, is noted as `fetch` API feedback in the fetch API
stabilization feedback (../../fetch/docs/stabilization.md).

### 2.3 TCP and flow-control policy

WinHTTP owns opaque sockets and exposes no raw socket handle, socket factory, `TCP_NODELAY`,
`SO_RCVBUF`, `SO_SNDBUF`, or initial-congestion-window option. `WinHttpOptions` does not imitate
these mechanisms.

`fetch` requires small writes to avoid Nagle/delayed-ACK stalls. A calibrated two-write experiment
shows the tested WinHTTP HTTP/1.1 upload path matching a raw `TCP_NODELAY` control rather than a
Nagle-enabled control. The transport therefore meets the behavioral invariant on the tested
platform even though WinHTTP does not document how it configures its socket. The experiment remains
regression evidence and is not presented as a Windows compatibility guarantee; the method and
measurements are recorded in the [Nagle behavior experiment](nagle-behavior-experiment.md).

WinHTTP exposes an HTTP/2 receive-window option, but the transport leaves it unset. Window sizing
trades path throughput against outstanding data per stream and is only one part of the OS flow-
control policy. Kernel socket buffers and TCP congestion startup likewise remain at OS defaults.
Application buffering inside the transport may still reduce callback, copy, and allocation
overhead; it is independent of these kernel and protocol controls.

## 3. HTTP protocol negotiation

The transport normally offers HTTP/1.1 and HTTP/2. Its composition builder may enable
`prefer_http3`, which allows WinHTTP to try HTTP/3 and fall back to the normal protocols.
This is a preference, never an HTTP/3 requirement.

Portable `fetch` protocol requirements take precedence. With no portable constraint,
`prefer_http3` offers HTTP/3, HTTP/2, and HTTP/1.1. An exact HTTP/2 requirement disables
HTTP/3 and sets `WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED`; an HTTP/1.1 requirement likewise
disables the newer protocols. A removed preference is not a conflict or construction
error.

The portable default is unconstrained rather than the closed set HTTP/1.1 and HTTP/2.
Without `prefer_http3`, the transport's own default still produces those two protocols.
There is no WinHTTP-specific `require_http3`; that requirement belongs in `fetch` only
after HTTP/3 joins the baseline supported by every transport.

Negotiation, including ALPN, is performed by the OS during the TLS handshake; the
transport does not negotiate manually. The version actually negotiated is reported on the
returned `HttpResponse`, so telemetry reflects what was negotiated rather than what was
requested. (How the version set is expressed to WinHTTP is implementation.md §10.1.)

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

- **Native automatic decompression is disabled.** WinHTTP returns the encoded body and
  its original headers. The mandatory `fetch` normalization layer advertises supported
  encodings and performs streaming decompression uniformly for every transport.
- **Request-body compression.** Not performed automatically; a caller that pre-encodes its
  body and sets `Content-Encoding` has it sent as-is.
- **Redirects are not followed.** Like `fetch_hyper` (and unlike WinHTTP's own default),
  3xx responses are surfaced to the caller unchanged rather than followed, with no knob
  to re-enable automatic redirects.
- **Cookies and automatic authentication are disabled.** The transport keeps no cookie
  store and does not answer `WWW-Authenticate`/407 challenges; `Set-Cookie`/`Cookie` and
  challenge responses pass through as plain data for the caller to manage. The transport
  is thus stateless between requests.

(The specific OS options behind each behavior are implementation.md §10.3.)

### 5.1 Full-duplex streaming and trailers

For HTTP/2, the send and receive sides progress independently. Response headers and body
data may become available before request upload completes, and `WinHttpWriteData` may
continue afterward. A direct experiment demonstrates both sequential interleaving and
overlapping send/receive operations on Windows 11 build 26100; the method and compatibility
limits are recorded in the
[full-duplex streaming experiment](full-duplex-streaming-experiment.md).

The implementation uses separate send and receive operation lanes on one request handle.
After response headers are returned, the response retains the shared request lifetime while
upload continues. A late upload failure remains visible through the response/request
completion contract rather than being dropped. Compatibility coverage must include every
supported Windows baseline because Microsoft documents concurrent send/receive support only
for "some versions of Windows."

`fetch` request bodies can declare an asynchronous, fallible terminal trailer result before
execution. WinHTTP has no public API for sending request trailers, so such a request fails
before its body is polled or any bytes are sent. WinHTTP can query response trailers after
body completion on the supported Windows baseline, including HTTP/1.1; those trailers are
returned as the terminal fallible response-body frame.

## 6. Timeouts and time

`fetch` enforces most timeouts above the transport; WinHTTP provides native
timers for the transport-owned steps. The transport owns exactly one timeout that
`fetch` does not enforce for us: connect.

### 6.1 Which timeouts the transport honors

- **Connect timeout** (`TransportOptions.connect_timeout`, default 30 s): honored by this
  transport as a *total* deadline on connection establishment (§6.2). `fetch` models this
  option but leaves each transport to enforce it. This is the sole source of the connect
  deadline: `WinHttpOptions` deliberately exposes no separate connect-timeout knob, so the
  two can never diverge.
- **Response timeout** (`http_extensions::ResponseTimeout`, read per-request from the
  request extensions): a *total* deadline over connection setup, sending the request, and
  receiving the response headers. `fetch` enforces this above the transport (the same way
  `fetch_hyper` relies on it), and it surfaces as `HttpError::timeout`. WinHTTP has no
  native timer with matching semantics - its receive-response timer covers only the
  post-send wait for the first response byte, excluding connect and send - so the transport
  does not remap `ResponseTimeout` onto a native timer; it only sets the native
  receive-response timer as a looser liveness backstop (implementation.md §10.4).
- **Body idle timeout** (`http_extensions::BodyTimeout`, read per-request from the request
  extensions): the maximum idle gap between response body chunks, reset on progress. This
  *does* match WinHTTP's `WINHTTP_OPTION_RECEIVE_TIMEOUT` (a per-receive-operation idle
  timer, reset each read), so the transport honors it natively by programming that timer
  per-request from the request's `BodyTimeout`. It surfaces as `HttpError::timeout`.
- **Seatbelt request timeout**: enforced above the transport; the transport is not
  involved.
- **Send timeout**: not a distinct concept. Sending the request and waiting for the
  response headers (without touching the body) is exactly the span `ResponseTimeout`
  already governs, after which `BodyTimeout` takes over; there is no separate send
  deadline to honor.
- **Resolve timeout**: `fetch` has no concept for a standalone DNS-resolution deadline
  (it is otherwise subsumed by `connect_timeout`), so it is exposed as a transport-specific
  `WinHttpOptions` knob for callers that need to bound resolution finely, as discussed in
  the fetch API stabilization feedback (../../fetch/docs/stabilization.md).

Cancellation is the transport's backstop for every timeout it does not enforce natively:
when `fetch` (or any layer above) drops the request future on a timeout, the transport
honors it by closing the WinHTTP handle and tearing the request down (the drop-safety
contract in §7 and implementation.md §4).

### 6.2 The outer connect timeout

WinHTTP's native connect timer bounds only a single per-address connection
*attempt*. A multi-homed host has several addresses that WinHTTP tries in turn,
retrying transient failures, so the *total* time to establish a connection can
exceed `TransportOptions.connect_timeout` even though every individual attempt
honored the native per-attempt timer. `fetch` callers expect `connect_timeout`
to be a *total* deadline, so the transport enforces the total itself, above the
native per-attempt timer (how it does so is implementation.md §4.6).

The deadline spans connection establishment: name resolution, TCP/TLS connect,
proxy discovery, and sending the request line and headers. The request body is
streamed afterward, so it lies outside this deadline and is governed by the
body idle timer (§6.1) instead.

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
  `REQUEST_ERROR` callback. `SECURE_FAILURE` supplies a bitmask of certificate
  problems that we capture and attach.
- **Mapping.** A Win32/`WINHTTP_*` code is turned into
  `HttpError::other(WinHttpError { code, .. }, recovery, label)`.
- **Labels** (mirroring `fetch`'s own error labels):

  | Condition | `ErrorLabel` |
  |-----------|--------------|
  | `ERROR_WINHTTP_CANNOT_CONNECT`, `NAME_NOT_RESOLVED` | `connect` |
  | `ERROR_WINHTTP_TIMEOUT` | `timeout` |
  | `ERROR_WINHTTP_SECURE_FAILURE` and secure-failure bits | `tls` |
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
any retry policy on them lives in `seatbelt` above the transport. Response
decompression occurs above this transport in `fetch`; only genuine wire/OS failures
enter this mapping.

[`RequestHandler`]: https://github.com/microsoft/oxidizer/tree/main/crates/http_extensions
[WinHTTP]: https://learn.microsoft.com/en-us/windows/win32/winhttp/using-winhttp
