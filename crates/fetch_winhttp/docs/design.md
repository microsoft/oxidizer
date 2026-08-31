# `fetch_winhttp` design

This document describes the user-visible behavior and design tenets of the
`fetch_winhttp` crate. The implementation strategy - threading, FFI ownership,
pooling, body-streaming mechanics, and the testing strategy - is documented separately
in [implementation.md](implementation.md). Runnable demonstrations of each feature
area live in `crates/fetch_winhttp/examples/`.

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
  connection pooling, keep-alive and automatic gzip/deflate decompression are
  handled by the OS.
- **Smaller dependency surface.** No rustls/aws-lc-rs/native-tls/hyper on the
  request path.

Out of scope: any non-Windows platform (the transport and the entire public API
are Windows-only; on other targets the crate still compiles, exposing nothing);
WebSocket upgrades; proxies (§2.3).

### 1.1 Constructing a client

A caller builds a WinHTTP-backed client the same way as the bundled Tokio transport,
except the constructors arrive through an extension trait this crate implements on
`fetch::HttpClient` (imported into scope):

```rust,ignore
use fetch::HttpClient;
use fetch_winhttp::{HttpClientWinHttpExt, WinHttpDeps, WinHttpTlsConfig};

// Clock, memory pool, and telemetry sink come from the application's environment.
// TLS configuration defaults when omitted.
let deps = WinHttpDeps::builder(clock, global_pool, sink)
    .tls(WinHttpTlsConfig::builder()
        .accept_invalid_certs(true)                 // Schannel knobs, §4
        .build())
    .build();

let client = HttpClient::builder_winhttp(deps)
    .build();                    // a `fetch::HttpClientBuilder`, so the pipeline can be tuned first
```

The result is an ordinary `fetch` `HttpClient`; no other caller code changes.
`WinHttpDeps` carries the mandatory environment dependencies needed by this transport:
the timer-capable `tick::Clock`, `bytesbuf::mem::GlobalPool`, and `observed::Sink`.
These values cannot be invented by the crate and therefore have no defaults. Its TLS
configuration is user configuration and does default. `WinHttpDeps` and its
component config types are `#[non_exhaustive]` and constructed through builders so new
fields can be added compatibly:

```rust,ignore
/// WinHTTP-specific dependencies. Construct with [`WinHttpDeps::builder`].
#[derive(thread_aware::ThreadAware)]
#[non_exhaustive]
pub struct WinHttpDeps { /* clock, global pool, sink, TLS - private */ }

impl WinHttpDeps {
    /// Starts building a `WinHttpDeps`.
    pub fn builder(
        clock: tick::Clock,
        global_pool: bytesbuf::mem::GlobalPool,
        sink: observed::Sink,
    ) -> WinHttpDepsBuilder;
}

/// Adds WinHTTP-transport constructors to `fetch::HttpClient`.
pub trait HttpClientWinHttpExt {
    /// Returns a builder for an `HttpClient` on the WinHTTP transport.
    fn builder_winhttp(deps: impl Into<WinHttpDeps>) -> HttpClientBuilder;
}
```

`WinHttpTlsConfig` (§4) follows the same builder + `#[non_exhaustive]` pattern.

### 1.2 TLS is configured on the transport, not through `fetch`'s `TlsOptions`

`fetch`'s generic `TlsOptions`/`TlsBackend` carries rustls/native-tls material
(crypto providers, verifiers, client-cert resolvers) that is meaningless to
Schannel. WinHTTP does TLS itself and accepts only a small set of knobs, so
`fetch_winhttp` therefore ignores `fetch`'s TLS configuration entirely and takes its
own `WinHttpTlsConfig` instead (§4). Different transports inherently support different TLS
configuration models, so trying to configure TLS uniformly at the transport-abstract
`fetch` level is over-abstraction on `fetch`'s part; see the fetch API stabilization
feedback (../../fetch/docs/stabilization.md).

### 1.3 Platform support

The transport requires Windows 11 version 21H2 (build 22000) or later, or
Windows Server 2025 (build 26100) or later. Windows Server 2022 (build 20348)
is not supported: the WinHTTP response-header query capabilities the transport
relies on are documented as introduced in build 22000, which Windows Server 2022
predates. The crate does not probe the OS build at session construction; on a
below-floor host the client still builds and failures surface later as ordinary
`request_winhttp` errors on the first request that needs the missing capability.

## 2. Connection management

WinHTTP owns connection establishment, pooling, keep-alive, and reuse; the transport
does not open, bind, or pool sockets itself. This chapter states the externally visible
connection guarantees and which generic `fetch` options are honored.

**Client isolation is guaranteed.** Independently built `HttpClient` values never reuse
each other's connections. This is a security boundary: a strict client and one built
with `accept_invalid_certs` (§4) cannot share an established TLS connection. The contract
does not specify how connections are organized or reused within one client.

### 2.1 Mapping generic transport options onto WinHTTP

The generic `TransportOptions`, `Http2Options`, and `TlsOptions` accepted by `fetch`
do not map one-to-one onto WinHTTP behavior, so some values are exact, some
approximate, and some ignored:

| `fetch` option | Contract |
|----------------|----------|
| `connect_timeout` | Honored as a total connection-establishment deadline (§6.2). |
| `request_filter` | Honored. |
| `supported_http_versions` | Honored for HTTP/1.1, HTTP/2, and HTTP/3; other versions are rejected. |
| `multiple_pools` | Accepted; its behavior remains defined by the generic `fetch` client contract. |
| `max_connections = usize::MAX` (default) | Honored as no caller-imposed limit. |
| finite `max_connections` | Ignored. |
| `connection_idle_timeout` | Honored, raised to a minimum of 5 seconds and approximated above roughly 49 days. Bounds how long an unused connection stays eligible for reuse; it does not promise prompt socket release. |
| `connection_lifetime = Unlimited` (default) | Honored. |
| `connection_lifetime = Fixed(_)` / `PerConnection(_)` | Ignored (§2.2). |
| `ConnectionKeepAlive::Disabled` (default) | Uses Windows defaults. |
| `ConnectionKeepAlive::ActiveConnections { interval, timeout }` | The interval is honored on HTTP/2, raised to a minimum of 5 seconds, and on HTTP/3, raised to a minimum of 1 millisecond. It does not apply to HTTP/1.1, which has no keep-alive probe to send. The generic `timeout` is ignored. |
| `ConnectionKeepAlive::ActiveAndIdleConnections { interval, timeout }` | Behaves like `ActiveConnections`; Windows does not distinguish these modes. |
| `Http2Options::initial_max_send_streams` | Ignored; Windows owns HTTP/2 stream concurrency. |
| `Http2Options::adaptive_window` | Ignored; Windows owns HTTP/2 flow control. |
| `TransportOptions::extra` | Ignored; no v1 WinHTTP extension types are defined in the generic extension map. |
| generic TLS `supported_http_versions` | Ignored; protocol selection comes from `TransportOptions::supported_http_versions`. |
| generic TLS `client_identity` | Ignored; client certificates are out of scope (§4.1). |
| generic TLS automatic/backend selection | Ignored; Schannel/WinHTTP is always the backend. |
| preconfigured rustls/native-tls backend | Ignored; those backend objects cannot configure WinHTTP. |
| rustls crypto provider or certificate verifier | Ignored; Schannel owns cryptography and certificate verification. |
| rustls client-certificate resolver | Ignored; client certificates are out of scope (§4.1). |

(The option mapping and the reasoning behind each floor are implementation.md §10.3.)

`ConnectionInfo` (age, `is_expired`, poisoning) that `fetch_hyper` attaches to
responses is not reproduced: this transport does not track the identity, age, or
health of individual connections, so a response from it carries no
`ConnectionInfo`.

### 2.2 Connection lifetime (bounded connection age)

`fetch`'s `connection_lifetime` option asks the client to stop reusing a
connection once it reaches a maximum age (`Fixed(d)`: every connection expires
after `d`; `PerConnection(f)`: a per-connection age drawn from `f`). The intent is
to bound how long any single TCP/TLS connection stays in service so long-lived
clients periodically re-establish connections (load-balancer rebalancing, cert
rotation, routing changes).

**v1 ignores `connection_lifetime` for `Fixed` and `PerConnection`.** The transport
does not track the identity or age of individual connections (§2.1), so a bounded
connection age is not part of its contract.

`connection_idle_timeout` is honored and bounds how long an unused connection stays
eligible for reuse. It does not bound the age of a continuously busy connection,
which is the gap a caller setting `connection_lifetime` should expect to remain
open.

Windows expresses this window as an unsigned millisecond count with no "never evict"
encoding, so `ConnectionIdleTimeout::Unlimited` and any window longer than roughly
49 days both become the longest window Windows can express. A caller asking for
indefinite retention gets a window long enough that no practical deployment reaches
it, but not a guarantee that idle eviction is disabled.

Unsupported generic connection options are ignored without runtime diagnostics. Their
fidelity is documented here so callers can select transport-specific configuration
knowingly.

### 2.3 Proxy support

Requests always connect directly to the origin. The transport does not use a proxy,
consult Windows proxy configuration, or run proxy auto-configuration scripts, and it
offers no setting that changes this.

The target scenario is service-to-service traffic, which reaches its peers directly.
Against that, proxy support costs every request: discovery is per-destination work
that a client doing nothing else of note pays on the request path. Declining it
outright is both simpler and faster than configuring it away.

A caller who needs a proxy is not served by this transport. Supporting one would be a
feature in its own right, with its own configuration surface, and is not planned.

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
requested. (How the version set is expressed to WinHTTP is implementation.md §10.1.)

### 3.1 The version on the request message

The version field of an `HttpRequest` does not select the wire version. Only the
configured version set above, plus negotiation, does:

- A request message whose version is `HTTP/0.9` or `HTTP/1.0` is rejected with an
  `invalid_request` error (§7) before anything is sent, because the transport cannot
  send those versions on the wire.
- Any other version on the request message is ignored. In particular, a request marked
  `HTTP/2` is not forced onto HTTP/2 and is not rejected when the configured version set
  excludes HTTP/2; it is sent over whatever the configured set and negotiation produce.

The version set and the request message's version are reported as separate conditions, so
an operator can tell from the error which of them needs correcting: the version set is
fixed on the client, the message version on the request.

## 4. TLS

TLS is handled by the OS through Schannel (§1); the transport ships no userland root
bundle and configures only a small set of `WinHttpTlsConfig` knobs (§1.2):

- **`https` selection.** `https://` targets use TLS. `http://` is issued only when the
  client is built with `insecure_allow_http()` and the request filter admits it -
  identical policy to the other transports.
- **Insecure mode.** `accept_invalid_certs` relaxes Schannel failures for an
  unknown CA, an invalid validity period, and an invalid intended usage.
  `accept_invalid_hostnames` relaxes certificate host-name mismatch failures.
  These options do not suppress every possible Schannel or certificate failure.
  They are opt-in and documented as dangerous.
- **Revocation checking.** Secure requests check the server certificate for
  revocation. `accept_invalid_certs` withdraws the check, because a certificate
  reached that way generally publishes no revocation endpoint and WinHTTP offers
  no way to forgive a check that cannot complete. `accept_invalid_hostnames`
  leaves it in place.
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
- **Request-response sequencing.** The request body is fully sent before response
  reception begins.
- **Trailers.** Response trailers exposed by WinHTTP are returned as `HttpBody` trailer
  frames rather than discarded. HTTP/1.1 supports trailer fields after a chunked body,
  but WinHTTP does not expose them; response trailers are therefore available only for
  HTTP/2 and HTTP/3. Outgoing trailer frames are unsupported and fail the request rather
  than being silently dropped. A trailer frame is reached only once the body yields it,
  so that failure arrives after the headers and every preceding data frame have been
  sent (§7).
- **`Transfer-Encoding` is rejected in request headers.** The transport derives request
  framing from the body itself, so a caller-supplied transfer coding fails the request
  with `invalid_request` (§7) before anything is sent. Removing the header does not change
  how the body is framed on the wire. Code that forwards an inbound request's headers
  verbatim is the common case that trips on this.
- **`Content-Length` must be a single well-formed value**, with repeated fields in
  agreement. A body that reports its own length is authoritative: the header must equal
  it, and a disagreement fails the request before anything is sent. A body that cannot
  report one is framed against the header on trust. A surviving header is sent in
  normalized decimal form.
- **Redirects are not followed.** Like `fetch_hyper` (and unlike WinHTTP's own default),
  3xx responses are surfaced to the caller unchanged rather than followed, with no knob
  to re-enable automatic redirects.
- **Cookies and automatic authentication are disabled.** The transport keeps no cookie
  store and does not answer `WWW-Authenticate`/407 challenges; `Set-Cookie`/`Cookie` and
  challenge responses pass through as plain data for the caller to manage. The transport
  is thus stateless between requests.

(The specific OS options behind each behavior are implementation.md §10.3.)

## 6. Timeouts and time

The transport honors timeout settings according to the caller-visible semantics below.
The enforcement mechanisms are documented in implementation.md §10.4.

### 6.1 Which timeouts the transport honors

- **Connect timeout** (`TransportOptions.connect_timeout`, default 30 s): honored as a
  total deadline on connection establishment (§6.2).
- **Response timeout** (`http_extensions::ResponseTimeout`, set per-request in the
  request extensions): a *total* deadline over connection setup, sending the request, and
  receiving the response headers. Expiration surfaces as `HttpError::timeout`. The
  `fetch` client pipeline enforces this deadline by wrapping the transport call; the
  transport neither reads the extension nor programs a native timer for it.
- **Body idle timeout** (`http_extensions::BodyTimeout`, read per-request from the request
  extensions): the maximum idle gap between response body frames, reset on progress.
- **Seatbelt request timeout**: honored by the client pipeline without transport-specific
  configuration.

Name resolution has no separate deadline. It is covered by the connect timeout, which
spans it along with the rest of connection establishment (§6.2).

### 6.2 Connect timeout scope

The deadline spans name resolution, TCP/TLS connection establishment, and sending the
request line and headers. The request body lies outside this deadline but
remains inside the per-request `ResponseTimeout`, which continues through the complete
upload and response headers (§6.1).

One consequence: the deadline can fire after the headers reached the server, so a
bodyless non-idempotent request may already be in processing when it trips.
Whether that request is safe to retry is `seatbelt`'s concern, not the
transport's; the transport only reports the timeout.

## 7. Error handling model

`fetch` transports return `Result<HttpResponse, HttpError>`. `HttpError`
(`http_extensions`) carries a source error, an `ohno::ErrorLabel`, and a
`recoverable::RecoveryInfo`, mirroring `fetch_hyper`:

- **Error surface.** A failure returns an `HttpError` carrying an `ohno::ErrorLabel`, a
  `recoverable::RecoveryInfo` classification, and a source error describing the failure.
- **Which errors state a Win32 code.** A failure that originates from a WinHTTP call -
  every `connect`, `timeout`, `tls`, `abandoned` and `winhttp_initialization` failure, and
  the `request_winhttp` failures reported by a WinHTTP call itself - carries a source error
  whose message states
  the originating Win32/`WINHTTP_*` code. Secure failures may additionally state a bitmask
  of certificate problems as best-effort diagnostics. The numeric code is diagnostic only:
  it is not programmatically accessible, so callers branch on the label and the recovery
  classification, never on a code.

  The families that state no code, because no WinHTTP call produced them, are:
  - a request the transport rejects itself, whose message states what the caller must
    change (`invalid_request`);
  - response metadata that a successful WinHTTP call returned but that cannot be parsed
    or represented, whose message describes the malformed value (`request_winhttp`);
  - an error raised by the caller's own request body stream, which is surfaced exactly as
    the caller's body produced it, with its own label and classification.

  The transport's connect deadline (§6.2) likewise carries no code: it states the
  configured deadline and is labeled `response_timeout`, the same expiry that a
  `ResponseTimeout` reports (§6.1).
- **Labels** (mirroring `fetch`'s own error labels):

  | Condition | `ErrorLabel` |
  |-----------|--------------|
  | the connection could not be established: the name did not resolve, or the peer refused the connection or could not be reached | `connect` |
  | a WinHTTP operation exceeded its own time limit | `timeout` |
  | TLS failed: certificate validation, revocation checking, secure-channel negotiation, or a client identity this transport cannot supply | `tls` |
  | the operation was cancelled or aborted, or WinHTTP shut down, rather than the request being allowed to fail | `abandoned` |
  | a send, receive, or protocol failure; a response exceeding a limit WinHTTP enforces; response metadata the transport cannot use; and any WinHTTP code the transport does not recognize | `request_winhttp` |
  | a request the transport rejects itself: an unusable HTTP version (§3, §3.1), an unusable target, or request body framing the transport cannot honor (§5) | `invalid_request` |
  | the WinHTTP session could not be opened or configured | `winhttp_initialization` |
  | the connect deadline expired (§6.2) | `response_timeout` |

  The table states which condition each label covers. The exact set of native codes
  recognized for a condition is not contractual; a code outside that set is labeled
  `request_winhttp` and carries unknown recovery guidance. A label follows the code
  WinHTTP reports rather than the underlying cause, so where one code spans several
  conditions the label reflects the code: a TLS incompatibility reported as a generic
  connection failure is labeled `request_winhttp`, not `tls`. Abortion is read by its
  subject: an aborted *operation* is `abandoned`, because something stopped the request
  deliberately, while an aborted *connection* is an ordinary transport fault and is
  labeled accordingly.
- **Transmission on `invalid_request`.** A rejection decided from request metadata
  happens before any WinHTTP call, so the server saw nothing. A body frame the transport
  cannot send is discovered only when the body yields it, by which point the headers and
  every preceding data frame have gone out. An `invalid_request` therefore does not on
  its own promise that the request had no remote effect.

- **Permanently failed initialization.** A transport that cannot open or configure its
  WinHTTP session latches that failure instead of retrying it. Every request it
  subsequently serves returns a fresh `winhttp_initialization` error without performing
  network I/O.

### 7.1 Recoverability rationale

`recoverable::RecoveryInfo` feeds `seatbelt`'s retry and breaker layers above the
transport. The division is not arbitrary; the rule is: an error is retryable iff
retrying the identical request (on a fresh connection) could plausibly succeed
without the caller changing anything. Idempotency and retry budgets are
`seatbelt`'s concern, not ours; we classify only whether the failure is transient
transport noise, a deterministic condition, or a code we do not recognize well enough
to say.

Which condition falls in which class is descriptive rather than contractual: the
mapping below reflects the transport's current judgement and may change.

- **Retryable** (transient transport/connection faults): a connection reset, aborted, or
  closed mid-flight; a name that did not resolve (DNS can be flaky); a peer that refused
  the connection or could not be reached (transient server/pool state); an operation that
  exceeded its time limit (transient load); a request WinHTTP asks to be resent; and a
  certificate-revocation check WinHTTP reports as unable to complete, which a retry may
  find the revocation service able to answer.
- **Never** (deterministic failures): TLS failures other than that incomplete revocation
  check, because a fixed trust configuration yields the same verdict on a retry, and
  because a client identity this transport cannot supply will still be missing; and an
  operation cancelled or aborted, or stopped by WinHTTP shutting down, which stop an
  operation rather than fail it, so re-issuing would work against whatever stopped it.
  Malformed responses, protocol violations, and responses exceeding a limit WinHTTP
  enforces are also non-retryable: each indicates a stable server or configuration
  problem.
- **Unknown** (everything else): the recognized codes are a small subset of the many
  codes WinHTTP can return, so an unrecognized code is the ordinary case rather
  than an exception. Such a failure carries unknown recovery guidance instead of being
  asserted retryable or never, leaving the decision to the policy layers above.

HTTP status codes (4xx/5xx) never enter this mapping: they are successful
transport outcomes carrying an error status, surfaced as `Ok(HttpResponse)`, and
any retry policy on them lives in `seatbelt` above the transport. Automatic
decompression handled by WinHTTP never surfaces as a transport error; only genuine
wire/OS failures do.

## 8. Telemetry

The transport reports through the `observed::Sink` supplied in `WinHttpDeps` (§1.1).
The event, counter, and field names below are a stable surface that dashboards and
alerts bind to; they are part of the contract, not incidental diagnostics.

| Event | Signal | Emitted when |
|-------|--------|--------------|
| `fetch.winhttp.session.initialization.failure` | log (error) | A transport instance cannot open or configure its WinHTTP session and becomes permanently failed (§7). |
| `fetch.winhttp.request.accepted` | metric | The transport accepts a request, before any of it is processed. |
| `fetch.winhttp.request.error` | log (error) + metric | A request returns `Err`. Requests that fail while the caller reads the response body are not counted, because the response was already returned successfully. |

Counters:

| Counter | Unit | Dimensions |
|---------|------|------------|
| `fetch.winhttp.request.accepted.count` | `{request}` | none |
| `fetch.winhttp.request.error.count` | `{error}` | none |

The counters are deliberately zero-dimensional. No per-request, per-connection, or
per-endpoint attribute is ever attached to a metric, so metric cardinality does not grow
with traffic, and the error counter divided by the request counter is the transport's
failure rate through the point where a response is returned. Failures a caller meets
later, while reading the response body, are outside both counters.

Log fields carry the higher-cardinality context instead, and appear on log records only:

| Event | Field | Value |
|-------|-------|-------|
| `fetch.winhttp.session.initialization.failure` | `winhttp.operation` | An identifier naming which step of session setup failed. The identifiers name internal setup steps and are diagnostic only; the set of them is not contractual. |
| `fetch.winhttp.session.initialization.failure` | `winhttp.error_code` | The Win32/`WINHTTP_*` code that step returned. |
| `fetch.winhttp.request.error` | `winhttp.connection.fresh` | Present and `true` only when the failed request began establishing a new physical connection rather than reusing a pooled one. A request that failed while still connecting carries the field, since that is the case the attribution most needs to identify. Absent otherwise. |
| `fetch.winhttp.request.error` | `winhttp.connect.duration` | Seconds spent on that connection attempt, measured until the request headers were sent or the send failed. Present under the same condition as `winhttp.connection.fresh`. |

Cold-connect attribution distinguishes "the server or pool is unhealthy" from
"establishing new connections is slow or failing", which have different remediations. It
stays log-only because connection-establishment state is per-request context, and
promoting it to a metric dimension would multiply the series count for no aggregate value.

[`RequestHandler`]: https://github.com/microsoft/oxidizer/tree/main/crates/http_extensions
[WinHTTP]: https://learn.microsoft.com/en-us/windows/win32/winhttp/using-winhttp
