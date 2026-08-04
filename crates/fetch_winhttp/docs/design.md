# `fetch_winhttp` design

This document describes the user-visible behavior and design tenets of the
`fetch_winhttp` crate. The implementation strategy - threading, FFI ownership,
pooling, body-streaming mechanics, and the test plan - is documented separately
in [implementation.md](implementation.md).

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
let deps = WinHttpDeps::builder(clock, global_pool, sink)
    .tls(WinHttpTlsConfig::builder()
        .accept_invalid_certs(true)                 // Schannel knobs, §4
        .build())
    .options(WinHttpOptions::builder()
        .resolve_timeout(Duration::from_secs(10))   // optional DNS-resolution deadline, §6
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

## Platform support

The transport requires Windows 11 version 21H2 (build 22000) or later. Earlier
Windows releases are not supported.

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
| `connection_idle_timeout` | Ignored; Windows manages idle connection retention. |
| `connection_lifetime = Unlimited` (default) | Honored. |
| `connection_lifetime = Fixed(_)` / `PerConnection(_)` | Ignored (§2.2). |
| `ConnectionKeepAlive::Disabled` (default) | Uses Windows defaults. |
| `ConnectionKeepAlive::ActiveConnections { interval, timeout }` | The interval is honored, raised to a minimum of 5 seconds for HTTP/2 and 1 millisecond for HTTP/3. The generic `timeout` is ignored; HTTP/1.1 has no equivalent behavior. |
| `ConnectionKeepAlive::ActiveAndIdleConnections { interval, timeout }` | Behaves like `ActiveConnections`; Windows does not distinguish the two modes. |
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
responses cannot be reproduced: WinHTTP hides individual connections, so per
connection age is not observable and no per-connection identity is exposed.
A response from this transport carries no `ConnectionInfo`.

### 2.2 Connection lifetime (bounded connection age)

`fetch`'s `connection_lifetime` option asks the client to stop reusing a
connection once it reaches a maximum age (`Fixed(d)`: every connection expires
after `d`; `PerConnection(f)`: a per-connection age drawn from `f`). The intent is
to bound how long any single TCP/TLS connection stays in service so long-lived
clients periodically re-establish connections (load-balancer rebalancing, cert
rotation, routing changes).

WinHTTP does not expose individual connections, so no available mechanism bounds
connection age faithfully. **v1 therefore ignores `connection_lifetime` for
`Fixed` and `PerConnection`.**

Unsupported generic connection options are ignored without runtime diagnostics. Their
fidelity is documented here so callers can select transport-specific configuration
knowingly.

### 2.3 Proxy discovery

The transport follows the current automatic Windows proxy policy, including automatic
discovery and PAC handling. v1 exposes no proxy configuration or direct-connection
override. This is a v1 design choice and may be revisited if future requirements call
for explicit proxy control.

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
  than being silently dropped.
- **Caller-supplied request framing headers.** A `Content-Length` supplied by the caller
  must be a single well-formed value - repeated fields must agree with each other - and
  must equal the actual length of the request body. A violation fails the request locally,
  before anything is sent; a `Content-Length` that survives is transmitted in normalized
  decimal form. A `Transfer-Encoding` supplied by the caller is rejected, because the
  transport performs request framing itself.
- **Divergence from `fetch_hyper`: a caller-supplied `Transfer-Encoding` is rejected
  rather than honored.** `fetch_hyper` honors `Transfer-Encoding: chunked` on a body of
  unknown length and sends the request; this transport fails such a request instead,
  whatever the length of the body, before anything reaches the network, with an
  `invalid_request` error (§7). The callers this affects are the ones that forward an
  inbound request's headers verbatim - proxy and gateway style code - because a
  forwarded request commonly carries `Transfer-Encoding: chunked` together with a body
  of unknown length, so identical caller code succeeds on `fetch_hyper` and fails here.
  Removing the header from the forwarded set makes the request acceptable and does not
  change how the body is framed on the wire, since the transport derives the framing from
  the body itself.
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
- **Response timeout** (`http_extensions::ResponseTimeout`, read per-request from the
  request extensions): a *total* deadline over connection setup, sending the request, and
  receiving the response headers. Expiration surfaces as `HttpError::timeout`.
- **Body idle timeout** (`http_extensions::BodyTimeout`, read per-request from the request
  extensions): the maximum idle gap between response body frames, reset on progress.
- **Seatbelt request timeout**: honored by the client pipeline without transport-specific
  configuration.
- **Send timeout**: not a distinct concept. Sending the request and waiting for the
  response headers (without touching the body) is exactly the span `ResponseTimeout`
  already governs, after which `BodyTimeout` takes over; there is no separate send
  deadline to honor.
- **Resolve timeout**: `fetch` has no concept for a standalone DNS-resolution deadline
  and therefore exposes `WinHttpOptions::resolve_timeout` as transport-specific
  configuration. It defaults to unlimited.

### 6.2 Connect timeout scope

The deadline spans name resolution, proxy discovery, TCP/TLS connection establishment,
and sending the request line and headers. The request body lies outside this deadline but
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
  `recoverable::RecoveryInfo` classification, and a source error whose message states the
  originating Win32/`WINHTTP_*` code. Secure failures may additionally state a bitmask of
  certificate problems as best-effort diagnostics. The numeric code is diagnostic only: it
  is not programmatically accessible, so callers branch on the label and the recovery
  classification, never on a code.
- **Labels** (mirroring `fetch`'s own error labels):

  | Condition | `ErrorLabel` |
  |-----------|--------------|
  | `ERROR_WINHTTP_CANNOT_CONNECT`, `NAME_NOT_RESOLVED` | `connect` |
  | `ERROR_WINHTTP_TIMEOUT` | `timeout` |
  | `ERROR_WINHTTP_SECURE_FAILURE` | `tls` |
  | `ERROR_WINHTTP_OPERATION_CANCELLED` | `abandoned` |
  | send/receive/protocol failures | `request_winhttp` |
  | a request rejected locally before it is sent: an unusable HTTP version (§3), an unusable target, or request body framing the transport cannot honor (§5) | `invalid_request` |
  | the WinHTTP session could not be opened | `winhttp_initialization` |

- **Permanently failed initialization.** A transport that cannot open its WinHTTP session
  latches that failure instead of retrying it. Every request it subsequently serves returns
  a fresh `winhttp_initialization` error without performing network I/O.

### 7.1 Recoverability rationale

`recoverable::RecoveryInfo` feeds `seatbelt`'s retry and breaker layers above the
transport. The division is not arbitrary; the rule is: an error is retryable iff
retrying the identical request (on a fresh connection) could plausibly succeed
without the caller changing anything. Idempotency and retry budgets are
`seatbelt`'s concern, not ours; we classify only whether the failure is transient
transport noise, a deterministic condition, or a code we do not recognize well enough
to say.

- **Retryable** (transient transport/connection faults): connection reset or
  closed mid-flight, `NAME_NOT_RESOLVED` (DNS can be flaky), `CANNOT_CONNECT`
  (transient server/pool state), `TIMEOUT` and `CONNECTION_ERROR` (transient
  load), and certificate-revocation checks that fail because the revocation
  server is unavailable. Re-issuing may land on a healthy connection or reach
  the external revocation service.
- **Never** (deterministic failures): TLS/certificate validation failures (given
  a fixed trust configuration and a completed revocation check, a retry yields
  the same verdict) and
  `OPERATION_CANCELLED` (the caller initiated teardown; retrying would contradict
  intent). Malformed-response/protocol violations that indicate a stable server
  or configuration problem are also non-retryable.
- **Unknown** (everything else): the recognized codes are a documented subset of the
  many codes WinHTTP can return, so an unrecognized code is the ordinary case rather
  than an exception. Such a failure carries unknown recovery guidance instead of being
  asserted retryable or never, leaving the decision to the policy layers above.

HTTP status codes (4xx/5xx) never enter this mapping: they are successful
transport outcomes carrying an error status, surfaced as `Ok(HttpResponse)`, and
any retry policy on them lives in `seatbelt` above the transport. Automatic
decompression handled by WinHTTP never surfaces as a transport error; only genuine
wire/OS failures do.

[`RequestHandler`]: https://github.com/microsoft/oxidizer/tree/main/crates/http_extensions
[WinHTTP]: https://learn.microsoft.com/en-us/windows/win32/winhttp/using-winhttp
