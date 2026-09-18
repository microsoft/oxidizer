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
Hyper composition transports.

Why a WinHTTP transport:

- **OS-managed TLS/trust.** WinHTTP terminates TLS through Schannel and uses the
  Windows certificate stores and system trust policy. Applications that must
  honor enterprise trust configuration or CTLs get that without bundling a userland
  TLS stack.
- **OS-managed protocol stack.** HTTP/1.1, HTTP/2 and HTTP/3 negotiation,
  connection pooling, and keep-alive are handled by the OS. Response decompression
  remains in `fetch` so content behavior is transport-independent.
- **Smaller dependency surface.** No rustls/aws-lc-rs/native-tls/hyper on the
  request path.

Out of scope: any non-Windows platform (the transport and the entire public API
are Windows-only; on other targets the crate still compiles, exposing nothing);
WebSocket upgrades; proxies (§2.3).

### 1.1 Constructing a client

The application configures an unbuilt WinHTTP transport and passes it to `fetch`:

```rust,ignore
use fetch::HttpClient;
use fetch_winhttp::WinHttpTransport;

let transport = WinHttpTransport::builder()
    .prefer_http3(true)
    .client_certificates(certificate_catalog)
    .build();

let client = HttpClient::builder(transport)
    .build()?;
```

The result is an ordinary `fetch::HttpClient`. `fetch` supplies per-instance clocks,
memory pools, telemetry, runtime affinity, and dispatch-pool identity through its
transport factory context. The WinHTTP builder contains only composition configuration.

The transport validates the final portable requirements when `HttpClientBuilder::build`
runs, then produces an isolated factory. The factory creates one WinHTTP session for
each runtime-thread and dispatch-pool partition. Session acquisition can still fail when
a partition materializes later; that partition becomes an explicit failed handler.

### 1.2 Transport-specific configuration

Schannel mechanisms, HTTP/3 preference, and certificate provisioning belong to the
WinHTTP composition builder. A dependency-light companion configuration crate is added
only when libraries demonstrate a need to modify a WinHTTP setting after transport
erasure. Rustls/native-tls objects are never accepted or ignored by this transport.

### 1.3 Platform support

The target contract requires Windows build 26100 or later (Windows 11 version 24H2 or
Windows Server 2025). Full-duplex send/receive behavior is empirically verified on this
build family, while Microsoft documents it only as available on "some versions of
Windows." Transport validation rejects older builds. The executable duplex probe remains
part of compatibility qualification for supported Windows updates. Resource failures
that occur while materializing a later isolated partition remain per-partition failures.

## 2. Connection management

WinHTTP owns connection establishment, pooling, keep-alive, and reuse; the transport
does not open, bind, or pool sockets itself. This chapter states the externally visible
connection guarantees and which generic `fetch` options are honored.

**Client isolation is guaranteed.** Independently built `HttpClient` values never reuse
each other's connections. This is a security boundary: a strict client and one built
with `accept_invalid_certs` (§4) cannot share an established TLS connection. The contract
does not specify how connections are organized or reused within one client.

### 2.1 Portable connection requirements

The transport honors the complete portable connection contract:

- total connect deadline;
- maximum idle age, accepting values whose WinHTTP representation still implies the bound;
- total concurrent connections per origin through `WINHTTP_OPTION_MAX_CONNS_PER_SERVER`;
- maximum connection lifetime through session-generation rollover;
- dispatch-pool isolation supplied by `fetch`.

A value WinHTTP cannot honor is rejected during validation. HTTP/2 flow-control,
socket-buffer, keep-alive-probe, and pool-internal knobs are not portable requirements and
are not received by this transport.

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

WinHTTP does not expose physical connection age, so the transport enforces maximum
lifetime with session generations. Once a generation reaches the configured age, new
requests use a fresh session while active requests drain on the old generation. Closing
the drained session retires all remaining pooled connections. Younger connections may be
retired early, which still satisfies the upper-bound contract.

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

### 2.4 TCP and flow-control policy

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
disables newer protocols. A removed preference is not a conflict.

The portable default is unconstrained rather than a closed protocol list. There is no
WinHTTP-specific `require_http3`; HTTP/3 becomes a portable requirement only after every
supported transport implements it.

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

### 4.1 Named client certificates

Named client-certificate authentication is part of the portable baseline. Libraries
select a logical credential role; the WinHTTP composition binds that role to a
Windows-store selector or imported certificate/private-key material.

When a server requests a certificate, the transport queries its acceptable issuer list,
selects a compatible binding, attaches the resulting `PCCERT_CONTEXT` through
`WINHTTP_OPTION_CLIENT_CERT_CONTEXT`, and retries without exposing the provisioning
modality to the library. Missing or ambiguous bindings fail transport validation or the
authentication attempt explicitly.

## 5. WinHTTP-managed HTTP behavior

The OS handles several HTTP behaviors internally. The transport configures each so it
behaves consistently with the rest of `fetch`:

- **Native automatic decompression is disabled.** The encoded body and original headers
  reach the fetch-level streaming decompression layer.
- **Request-body compression.** Not performed automatically; a caller that pre-encodes its
  body and sets `Content-Encoding` has it sent as-is.
- **Full duplex.** For HTTP/2, response headers and body data may arrive while a known- or
  unknown-length upload continues. The send and receive lanes have independent operation
  slots and share one cancellation lifetime.
- **Trailers.** Response trailers are queried after EOF and returned as terminal body
  frames for every protocol on which WinHTTP exposes them, including HTTP/1.1 on the
  supported platform. A request declaring trailers is rejected during preflight before
  its body is polled or request bytes are sent, because WinHTTP has no send API for them.
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
any retry policy on them lives in `seatbelt` above the transport. Response
decompression occurs above this transport in `fetch`; only genuine wire/OS failures
enter this mapping.

## 8. Telemetry

The transport reports through the `observed::Sink` supplied by the per-instance transport
context (§1.1).
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
