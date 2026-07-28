<!-- Copyright (c) Microsoft Corporation. Licensed under the MIT License. -->

# WinHTTP capabilities reference

A factual reference for how WinHTTP handles connections, pooling, keep-alive, and
TLS, plus the concrete `WinHttpSetOption` inventory the design relies on. The
[`configuration-mapping.md`](configuration-mapping.md) doc translates fetch's knobs
onto these; this doc is the underlying source material.

Option names below are from the Microsoft Learn "Option flags (Winhttp.h)"
reference. Anything marked *undocumented* is observed behavior, not contract.

## Handle model

```
WinHttpOpen()        -> session handle    (proxy, timeouts, option state, the pool)
WinHttpConnect()     -> connection handle (records host:port; opens NO socket)
WinHttpOpenRequest() -> request handle    (per request)
```

Key subtlety: **`WinHttpConnect` does not open a socket.** It only records the
target. The actual TCP+TLS connection is established lazily on
`WinHttpSendRequest`. So the connection handle is largely nominal — it is not the
object that owns a pooled socket.

## Connection pooling

- **The pool is session-scoped.** Persistent connections are pooled at the
  `WinHttpOpen` session level, keyed by target host/port **plus** the security /
  proxy / client-cert context. Sockets are reused across request handles and even
  across different `WinHttpConnect` handles under the same session.
- **Keep-alive (reuse) is on by default.** A new request reuses an idle pooled
  socket to the same endpoint before dialing. `WINHTTP_DISABLE_KEEP_ALIVE` (via
  `WINHTTP_OPTION_DISABLE_FEATURE` on the request handle) forces `Connection:
  close` and opts the request out of pooling.
- **Consequence for emulation:** because the pool hangs off the *session* and
  `WinHttpConnect` owns no socket, rotating a connection handle does **not** flush
  pooled sockets. Forcing reconnection is done per-request via
  `WINHTTP_OPTION_EXPIRE_CONNECTION` (below), or per-session by recycling the
  session handle.

## Idle connections

- **Built-in idle scavenger, ~1 minute (undocumented, untunable).** An idle
  pooled socket unused for roughly a minute is closed and not reused. There is no
  API to change this value.
- **No built-in max-lifetime.** A connection reused within the idle window can
  live indefinitely unless explicitly retired.

### Alignment with hyper defaults

fetch-over-hyper's defaults (`fetch_options` `DEFAULT_POOL_LIFETIME`) are: **60s
idle eviction, unlimited connections per host, no max-lifetime**. (Note
`hyper-util`'s own native idle default is 90s, but fetch always overrides it to
60s.) WinHTTP's ~60s scavenger and "no max-lifetime" default line up almost
exactly, so a caller who never sets these gets equivalent behavior on both
transports with **zero emulation**. Emulation is only needed for non-default
values.

### Idle timeout: shorten vs. prolong asymmetry

| Direction | WinHTTP support | Mechanism |
| --- | --- | --- |
| **shorten** (< ~60s) | exact | eager close via `WINHTTP_OPTION_EXPIRE_CONNECTION`, or app-level early close |
| **match** (~60s) | native | the built-in scavenger |
| **prolong, HTTP/2** (> ~60s) | native | `WINHTTP_OPTION_HTTP2_KEEPALIVE` — idle PING frames count as activity, so the connection stays alive |
| **prolong, HTTP/1.1** (> ~60s) | not natively supported | only a crude app-level "warmer" (periodic real request); TLS session resumption softens the reconnect cost, so this is rarely needed |

The safe/common directions (shorten, match, or prolong-over-H2) are exact or
native; only prolonging an idle **HTTP/1.1** connection lacks a real mechanism.

## Keep-alive: three distinct mechanisms

"Keep-alive" conflates three layers. fetch's `ConnectionKeepAlive` is **only** the
third.

1. **HTTP/1.1 persistent connections** (`Connection: keep-alive`) — reuse one TCP
   connection for sequential requests. On by default; disable via
   `WINHTTP_DISABLE_KEEP_ALIVE`.
2. **TCP keep-alive** (`SO_KEEPALIVE`) — OS-level empty-ACK probes to detect a dead
   peer and hold NAT state. OS/registry-managed; does **not** reset WinHTTP's
   HTTP-level idle scavenger (wrong layer).
3. **HTTP/2 keep-alive (PING frames)** — application-level PINGs on a live H2
   connection to health-check it and hold it open through idle-killing
   intermediaries. **This is what fetch's `ConnectionKeepAlive` configures.**

WinHTTP exposes mechanism 3 via `WINHTTP_OPTION_HTTP2_KEEPALIVE` (session handle):
an idle-triggered timeout in milliseconds (floor 5000ms) after which WinHTTP sends
H2 PING frames. There is a matching `WINHTTP_OPTION_HTTP3_KEEPALIVE`. WinHTTP does
**not** expose the individual PING *interval* / ACK-*timeout* / active-only
distinctions that hyper does.

## TLS / certificates

- **SChannel + Windows trust stores.** No userspace verifier hook; validation is
  the OS's, against the machine/user cert stores.
- **mTLS client identity**: `WINHTTP_OPTION_CLIENT_CERT_CONTEXT` with a Windows
  `CERT_CONTEXT`. For mTLS **over HTTP/2**, `WINHTTP_OPTION_ENABLE_HTTP2_PLUS_CLIENT_CERT`
  must also be set on the session.
- **Server cert inspection / pinning**: retrieve `WINHTTP_OPTION_SERVER_CERT_CONTEXT`
  (or `WINHTTP_OPTION_SECURITY_CERTIFICATE_STRUCT`) in the status callback to
  implement pinning on top of OS validation.
- **Revocation**: `WINHTTP_ENABLE_SSL_REVOCATION` (via `WINHTTP_OPTION_ENABLE_FEATURE`,
  request handle) — must be set explicitly; SChannel under WinHTTP does not
  hard-fail on revocation by default. The design treats revocation as an always-on
  invariant, so this is set unconditionally.
- **Protocol/version**: `WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL` bitmask —
  `WINHTTP_PROTOCOL_FLAG_HTTP2` (0x1), `WINHTTP_PROTOCOL_FLAG_HTTP3` (0x2); `0x0`
  restricts to HTTP/1.1 and prior. HTTP/1.1 and earlier **cannot** be disabled via
  this option (so a strict "HTTP/2 only, forbid H1" cannot be fully enforced).
  `WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED` can require the negotiated advanced
  version.

## Option inventory used by the design

| Purpose | Option | Handle | Notes |
| --- | --- | --- | --- |
| Per-server connection cap | `WINHTTP_OPTION_MAX_CONNS_PER_SERVER` | session | Default `INFINITE`; setting `0` caps at **2**. `..._PER_1_0_SERVER` for HTTP/1.0. |
| **Max-lifetime (native)** | `WINHTTP_OPTION_EXPIRE_CONNECTION` | active request | Closes the connection serving this request after it completes → per-connection forced retirement. The native max-lifetime primitive. |
| Idle H2 keep-alive | `WINHTTP_OPTION_HTTP2_KEEPALIVE` | session | Idle-triggered PING; ms, floor 5000. `..._HTTP3_KEEPALIVE` for H3. |
| Disable connection reuse | `WINHTTP_DISABLE_KEEP_ALIVE` (via `WINHTTP_OPTION_DISABLE_FEATURE`) | request | Forces `Connection: close`. |
| Stale-connection auto-retry | `WINHTTP_OPTION_FAILED_CONNECTION_RETRIES` (`WINHTTP_CONNECTION_RETRY_CONDITION_STALE_CONNECTION`) | session | Native retry when a reused connection turns out stale — mitigates idle-staleness. |
| Connection targeting | `WINHTTP_OPTION_CONNECTION_GUID` + `WINHTTP_OPTION_MATCH_CONNECTION_GUID` | request | Tag connections and pin requests to a connection group — enables per-group control the opaque pool otherwise hides. |
| mTLS identity | `WINHTTP_OPTION_CLIENT_CERT_CONTEXT` | request | + `WINHTTP_OPTION_ENABLE_HTTP2_PLUS_CLIENT_CERT` (session) for H2. |
| Server cert (pinning) | `WINHTTP_OPTION_SERVER_CERT_CONTEXT` | request/callback | Inspect in the status callback. |
| Revocation (always on) | `WINHTTP_ENABLE_SSL_REVOCATION` (via `WINHTTP_OPTION_ENABLE_FEATURE`) | request | Set unconditionally (invariant). |
| HTTP version enable | `WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL` | request/session | H2 / H3 flags; H1 cannot be disabled. |
| H2 receive window | `WINHTTP_OPTION_HTTP2_RECEIVE_WINDOW` | — | Partial analogue to hyper flow-control tuning. |
| Timeouts | `WinHttpSetTimeouts(resolve, connect, send, receive)` | session | `connect_timeout` maps to resolve+connect; send/receive left permissive (pipeline owns end-to-end deadlines). |
| Async mode + callback | `WINHTTP_FLAG_ASYNC`, `WinHttpSetStatusCallback`, `WINHTTP_OPTION_CONTEXT_VALUE` | session/request | Basis of the async bridge (see `async-bridge.md`). |

## Corrections this audit made to earlier design notes

Recorded so the reasoning trail is clear:

1. **Keep-alive is not purely hyper-specific.** `WINHTTP_OPTION_HTTP2_KEEPALIVE`
   exists, so the disable and idle-keepalive cases map (coarsely); only PING
   interval/ACK-timeout/active-only stay hyper-specific.
2. **Idle timeout can be prolonged over HTTP/2** natively (via H2 keep-alive PINGs)
   — not only shortened. The "no way to prolong" note applies to HTTP/1.1 only.
3. **Max-lifetime is native, not emulated.** `WINHTTP_OPTION_EXPIRE_CONNECTION`
   retires a connection per request-handle, replacing the earlier
   session-recycling emulation proposal.
4. **The pool is targetable.** `WINHTTP_OPTION_CONNECTION_GUID` defeats the
   "cannot address a specific pooled socket" limitation earlier assumed.
