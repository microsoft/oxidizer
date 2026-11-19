# `fetch_winhttp` design

This document defines the public behavior of `fetch_winhttp`.
[implementation.md](implementation.md) describes threading, FFI ownership,
pooling, body streaming, and testing. The
`crates/fetch_winhttp/examples/` directory contains runnable examples.

## 1. Purpose and scope

`fetch_winhttp` is a Windows-only transport for the [`fetch`] HTTP client. It
uses the operating system's [WinHTTP] client API in asynchronous mode. It is an
alternative to the bundled `fetch_hyper` transport.

The transport provides these properties:

- WinHTTP uses Schannel, Windows certificate stores, and the system trust
  policy. Applications can use enterprise trust configuration and CTLs without
  another TLS stack. Client certificates are not supported in v1 (§4.1).
- Windows manages HTTP/1.1, HTTP/2, HTTP/3, connection pooling, keep-alive, and
  gzip or deflate decompression.
- The request path does not use rustls, aws-lc-rs, native-tls, or hyper.

The transport does not support non-Windows platforms, WebSocket upgrades, or
proxies (§2.3). The crate compiles on other platforms but exposes no public API.

### 1.1 Constructing a client

Import the extension trait and call its constructor on `fetch::HttpClient`:

```rust,ignore
use fetch::HttpClient;
use fetch_winhttp::{HttpClientWinHttpExt, WinHttpDeps, WinHttpTlsConfig};

// The application supplies the clock, memory provider, and telemetry sink.
// TLS uses its default configuration when omitted.
let deps = WinHttpDeps::builder(clock, memory, sink)
    .tls(WinHttpTlsConfig::builder()
        .accept_invalid_certs(true)                 // Schannel knobs, §4
        .build())
    .build();

let client = HttpClient::builder_winhttp(deps) // Returns a configurable `fetch::HttpClientBuilder`.
    .build();
```

The result is a `fetch::HttpClient`. `WinHttpDeps` contains the required
`tick::Clock`, `bytesbuf::mem::OpaqueMemory`, and `observed::Sink`. These values
have no defaults. The TLS configuration has a default. `WinHttpDeps` and its
configuration types are `#[non_exhaustive]` and use builders:

```rust,ignore
/// WinHTTP-specific dependencies. Construct with [`WinHttpDeps::builder`].
#[derive(thread_aware::ThreadAware)]
#[non_exhaustive]
pub struct WinHttpDeps { /* clock, memory, sink, TLS - private */ }

impl WinHttpDeps {
    /// Starts building a `WinHttpDeps`.
    pub fn builder(
        clock: tick::Clock,
        memory: impl Into<bytesbuf::mem::OpaqueMemory>,
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

The memory provider allocates body buffers. Pass `GlobalPool::new()` directly,
or wrap a custom `MemoryShared` provider with `OpaqueMemory::new(provider)`.
The transport uses the provider for body construction, response reads, and
writer reservations. Request bodies can use other providers.

Cloning `WinHttpDeps` clones the provider. [Relocation] notifies the provider on
the destination thread. The provider controls whether clones share state. Its
block size does not change read or write limits.

### 1.2 TLS is configured on the transport, not through `fetch`'s `TlsOptions`

Generic `TlsOptions` and `TlsBackend` contain rustls or native-tls values that
cannot configure Schannel. The transport ignores them and uses
`WinHttpTlsConfig` (§4). See the [fetch API stabilization feedback] for the
generic API discussion.

### 1.3 Platform support

The transport requires Windows 11 version 21H2 (build 22000) or later, or
Windows Server 2025 (build 26100) or later. Windows Server 2022 (build 20348)
is not supported. The required WinHTTP response-header query was introduced in
build 22000. The crate does not inspect the OS build when it creates a session.
On an older host, the client can build. The first request that needs the missing
operation returns `request_winhttp`.

## 2. Connection management

WinHTTP manages connection establishment, pooling, keep-alive, and reuse. The
transport does not manage sockets. This section defines the public connection
behavior and supported generic configuration.

Independently built `HttpClient` values do not share connections. This keeps a
strict client separate from a client that uses `accept_invalid_certs` (§4). The
contract does not define connection reuse inside one client.

### 2.1 Mapping generic transport options onto WinHTTP

Some generic `TransportOptions`, `Http2Options`, and `TlsOptions` do not map to
WinHTTP. The following table defines each result:

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

Implementation details for these values are in implementation.md §10.3.

Responses do not contain `ConnectionInfo`. The transport does not track the
identity, age, or health of individual connections.

### 2.2 Connection lifetime (bounded connection age)

`connection_lifetime` sets a maximum connection age. `Fixed(d)` uses one age
for every connection. `PerConnection(f)` selects an age for each connection.
This can refresh load-balancer routing and certificates.

Version 1 ignores `Fixed` and `PerConnection`. The transport does not track the
identity or age of individual connections (§2.1).

`connection_idle_timeout` is honored and bounds how long an unused connection stays
eligible for reuse. It does not bound the age of a continuously busy connection,
which is the gap that remains when a caller sets `connection_lifetime`.

Windows stores this window as an unsigned millisecond count and has no
"never evict" value. `ConnectionIdleTimeout::Unlimited` and values over about
49 days use the largest value. This does not disable idle eviction.

Unsupported generic connection configuration is ignored without a runtime
diagnostic.

### 2.3 Proxy support

Requests connect directly to the origin. The transport does not use Windows
proxy configuration or proxy auto-configuration scripts. No configuration
value changes this behavior.

The target is direct service-to-service traffic. Proxy discovery adds
per-destination work to the request path.

Use another transport if the application needs a proxy. Proxy support is not
planned.

## 3. HTTP protocol negotiation

The transport supports HTTP/1.1, HTTP/2, and HTTP/3.
`TransportOptions.supported_http_versions` controls the allowed versions:

- An empty list uses the `fetch` default of HTTP/1.1 and HTTP/2.
- A list without HTTP/1.1 disables the HTTP/1.1 fallback. The request fails if
  it cannot negotiate one of the listed newer protocols.
- `HTTP/0.9` and `HTTP/1.0` return `invalid_request` during request
  construction.

Windows performs negotiation, including ALPN, during the TLS handshake. The
returned `HttpResponse` contains the negotiated version. Implementation details
are in implementation.md §10.1.

### 3.1 The version on the request message

The `HttpRequest` version does not select the wire version. The configured
version list and negotiation select it:

- `HTTP/0.9` and `HTTP/1.0` return `invalid_request` (§7) before the transport
  sends data.
- The transport ignores every other request-message version. For example, an
  `HTTP/2` message does not require HTTP/2.

Errors identify whether the client version list or the request-message version
must change.

## 4. TLS

Schannel handles TLS (§1). The transport does not include a root bundle. Use
`WinHttpTlsConfig` for the supported configuration (§1.2):

- `https://` targets use TLS. An `http://` request requires
  `insecure_allow_http()` and approval from the request filter.
- `accept_invalid_certs` relaxes Schannel failures for an
  unknown CA, an invalid validity period, and an invalid intended usage.
  `accept_invalid_hostnames` relaxes certificate host-name mismatch failures.
  These options do not suppress every possible Schannel or certificate failure.
  They are opt-in.
- Secure requests check the server certificate for
  revocation. `accept_invalid_certs` withdraws the check, because a certificate
  reached that way generally publishes no revocation endpoint and WinHTTP offers
  no way to forgive a check that cannot complete. `accept_invalid_hostnames`
  leaves it in place.
- Version 1 does not support server certificate inspection or pinning.

Implementation details are in implementation.md §10.2.

### 4.1 Client certificates (mTLS) are out of scope for v1

`fetch` does not require client certificate support. Its
`fetch::tls::ClientIdentity` value is part of the generic `TlsOptions`, which
this transport ignores (§1.2). Version 1 does not implement client certificates,
and `WinHttpTlsConfig` has no client-identity field.

## 5. WinHTTP-managed HTTP behavior

Windows handles several HTTP behaviors. The transport configures them as
follows:

- The transport always advertises
  `Accept-Encoding: gzip, deflate`. It decodes gzip and deflate responses and
  removes `Content-Encoding` and `Content-Length`. Version 1 has no opt-out
  because downstream code does not decode response bodies.
- Brotli and zstd responses are not decoded. They arrive
  still-encoded with `Content-Encoding` intact and pass through verbatim.
- Request bodies are not compressed automatically. A pre-encoded body and its
  `Content-Encoding` are sent unchanged.
- The transport sends the full request body before response
  reception begins.
- WinHTTP response trailers are returned as `HttpBody` trailer
  frames rather than discarded. HTTP/1.1 supports trailer fields after a chunked body,
  but WinHTTP does not expose them; response trailers are therefore available only for
  HTTP/2 and HTTP/3. Outgoing trailer frames are unsupported and fail the request rather
  than being silently dropped. A trailer frame is reached only once the body yields it,
  so that failure arrives after the headers and every preceding data frame have been
  sent (§7).
- A caller-supplied `Transfer-Encoding` is rejected. The transport derives request
  framing from the body itself, so a caller-supplied transfer coding fails the request
  with `invalid_request` (§7) before anything is sent. Removing the header does not change
  how the body is framed on the wire. Code that forwards an inbound request's headers
  verbatim is the common case that trips on this.
- `Content-Length` must be a well-formed value. Repeated fields must agree.
  A body that reports its own length is authoritative. The header must match it,
  or the request fails before the transport sends data. If the body does not
  report a length, the transport trusts the header. The transport sends a
  normalized decimal value.
- Redirects are not followed. A 3xx response is returned unchanged.
- Cookies and automatic authentication are disabled. The transport keeps no cookie
  store and does not answer `WWW-Authenticate`/407 challenges; `Set-Cookie`/`Cookie` and
  challenge responses pass through as plain data for the caller to manage. The transport
  is thus stateless between requests.

Implementation details are in implementation.md §10.3.

## 6. Timeouts and time

The transport uses the following timeout behavior. Implementation details are
in implementation.md §10.4.

### 6.1 Which timeouts the transport honors

- `TransportOptions.connect_timeout` defaults to 30 seconds. It is the total
  deadline for connection establishment (§6.2).
- `http_extensions::ResponseTimeout` is a per-request total deadline for
  connection setup, request sending, and
  receiving the response headers. Expiration surfaces as `HttpError::timeout`. The
  `fetch` client pipeline enforces this deadline by wrapping the transport call; the
  transport neither reads the extension nor programs a native timer for it.
- `http_extensions::BodyTimeout` is the maximum idle time between response body
  frames. Each frame resets the timer.
- The client pipeline enforces the Seatbelt request timeout without
  transport-specific configuration.

Name resolution has no separate deadline. It is covered by the connect timeout, which
spans it along with the rest of connection establishment (§6.2).

### 6.2 Connect timeout scope

The connect deadline covers name resolution, TCP or TLS connection
establishment, and request headers. It does not cover the request body. The
per-request `ResponseTimeout` continues through the upload and response headers
(§6.1).

The deadline can expire after the server receives the headers. A bodyless,
non-idempotent request can already be in progress. Seatbelt decides whether a
retry is safe. The transport reports only the timeout.

## 7. Error handling model

`fetch` transports return `Result<HttpResponse, HttpError>`. `HttpError`
contains a source error, `ohno::ErrorLabel`, and
`recoverable::RecoveryInfo`:

- A failure returns an `HttpError` with an `ohno::ErrorLabel`,
  `recoverable::RecoveryInfo` classification, and a source error describing the failure.
- A failure from a WinHTTP call includes the Win32 or `WINHTTP_*` code in the
  source error. This applies to every `connect`, `timeout`, `tls`, `abandoned`,
  and `winhttp_initialization` failure. It also applies to
  `request_winhttp` failures from a WinHTTP call. Secure failures can include a
  certificate-problem bitmask. The numeric code is diagnostic only. Callers
  use the label and recovery classification for program logic.

  These failures do not include a code because no WinHTTP call produced them:
  - a request the transport rejects itself, whose message states what the caller must
    change (`invalid_request`);
  - response metadata that a successful WinHTTP call returned but that cannot be parsed
    or represented, whose message describes the malformed value (`request_winhttp`);
  - an error raised by the caller's own request body stream, which is surfaced exactly as
    the caller's body produced it, with its own label and classification.

  The connect deadline (§6.2) also has no code. It states the
  configured deadline and is labeled `response_timeout`, the same expiry that a
  `ResponseTimeout` reports (§6.1).
- Labels:

  | Condition | `ErrorLabel` |
  |-----------|--------------|
  | the connection failed: the name did not resolve, or the peer refused the connection or was unreachable | `connect` |
  | a WinHTTP operation exceeded its own time limit | `timeout` |
  | TLS failed: certificate validation, revocation checking, secure-channel negotiation, or a client identity this transport cannot supply | `tls` |
  | the operation was cancelled or aborted, or WinHTTP shut down, rather than the request being allowed to fail | `abandoned` |
  | a send, receive, or protocol failure; a response exceeding a limit WinHTTP enforces; response metadata the transport cannot use; and any WinHTTP code the transport does not recognize | `request_winhttp` |
  | a request the transport rejects itself: an unusable HTTP version (§3, §3.1), an unusable target, or request body framing the transport cannot honor (§5) | `invalid_request` |
  | the transport failed to open or configure the WinHTTP session | `winhttp_initialization` |
  | the connect deadline expired (§6.2) | `response_timeout` |

  The exact set of native codes for each condition is not part of the contract.
  A code outside that set is labeled
  `request_winhttp` and carries unknown recovery guidance. A label follows the code
  WinHTTP reports rather than the underlying cause, so where one code spans several
  conditions the label reflects the code: a TLS incompatibility reported as a generic
  connection failure is labeled `request_winhttp`, not `tls`. Abortion is read by its
  subject: an aborted *operation* is `abandoned`, because something stopped the request
  deliberately, while an aborted *connection* is an ordinary transport fault and is
  labeled accordingly.
- A metadata rejection returns `invalid_request` before any WinHTTP call, so the
  server receives nothing. A rejection from a body frame happens after the
  transport sends the headers and earlier data frames. The label does not prove
  that the request had no remote effect.
- If session initialization fails, the transport stores the failure. Every
  later request returns a new `winhttp_initialization` error without network
  I/O.

### 7.1 Recoverability rationale

`recoverable::RecoveryInfo` supplies information to Seatbelt retry and breaker
layers. An error is retryable when the same request on a new connection can
succeed without caller changes. Seatbelt controls idempotency and retry budgets.
The transport classifies the failure as transient, deterministic, or unknown.

This mapping is not part of the contract and can change:

- Retryable failures include connection resets, connection aborts, connection
  closures, DNS failures, refused or unreachable peers, operation timeouts,
  resend requests from WinHTTP, and incomplete revocation checks.
- Non-retryable failures include other TLS failures, missing client identities,
  cancellation, WinHTTP shutdown, malformed responses, protocol violations,
  and responses over a WinHTTP limit.
- Every unrecognized code has unknown recovery guidance. Policy layers decide
  whether to retry it.

HTTP 4xx and 5xx statuses return `Ok(HttpResponse)`. Seatbelt controls retry
policy for these statuses. Automatic decompression does not return a transport
error. Only wire or operating-system failures do.

## 8. Telemetry

The transport reports through the `observed::Sink` in `WinHttpDeps` (§1.1).
The following event, counter, and field names are part of the contract:

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

The counters have no dimensions. They do not include request, connection, or
endpoint attributes. Divide the error count by the request count to calculate
the failure rate before a response is returned. Body-read failures occur after
that point and are outside both counters.

Log records contain the higher-cardinality context:

| Event | Field | Value |
|-------|-------|-------|
| `fetch.winhttp.session.initialization.failure` | `winhttp.operation` | An identifier naming which step of session setup failed. The identifiers name internal setup steps and are diagnostic only; the set of them is not contractual. |
| `fetch.winhttp.session.initialization.failure` | `winhttp.error_code` | The Win32/`WINHTTP_*` code that step returned. |
| `fetch.winhttp.request.error` | `winhttp.connection.fresh` | Present and `true` only when the failed request began establishing a new physical connection rather than reusing a pooled one. A request that failed while still connecting carries the field, since that is the case the attribution most needs to identify. Absent otherwise. |
| `fetch.winhttp.request.error` | `winhttp.connect.duration` | Seconds spent on that connection attempt, measured until the request headers were sent or the send failed. Present under the same condition as `winhttp.connection.fresh`. |

Cold-connect fields separate server or pool failures from slow or failed new
connections. They remain log-only because the state belongs to one request.

[Relocation]: https://docs.rs/thread_aware/latest/thread_aware/#theory-of-operation
[fetch API stabilization feedback]: ../../fetch/docs/stabilization.md
[`RequestHandler`]: https://github.com/microsoft/oxidizer/tree/main/crates/http_extensions
[WinHTTP]: https://learn.microsoft.com/en-us/windows/win32/winhttp/using-winhttp
