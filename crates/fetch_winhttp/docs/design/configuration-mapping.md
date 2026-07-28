<!-- Copyright (c) Microsoft Corporation. Licensed under the MIT License. -->

# Configuration mapping onto WinHTTP

> **Read [`configuration-model.md`](configuration-model.md) first** for the bucket
> model (A pipeline / B required-portable / B2 optional-portable / C
> transport-specific), and [`winhttp-capabilities.md`](winhttp-capabilities.md)
> for the underlying WinHTTP behavior and option inventory. This doc translates
> the **portable** buckets (B / B2) onto WinHTTP mechanisms and records which
> hyper-shaped knobs are Bucket C (and why).

Only Bucket B / B2 knobs reach a transport, so those are the only ones WinHTTP has
to map. Bucket A never leaves the pipeline; Bucket C is set on each transport's
own builder and never handed to a foreign transport. This is what dissolves the
old "mismatch table": WinHTTP is only asked to honor knobs it can.

## Verdict vocabulary

- **Honor** — a faithful WinHTTP equivalent exists; translate it.
- **Honor (coarse)** — WinHTTP honors the intent at coarser granularity; the gap
  is documented.
- **Fail fast** — for a security-relevant knob WinHTTP cannot honor,
  `WinHttpTransport::builder().build()` returns an error rather than silently
  proceeding.

There is no "silently approximate" verdict: a knob either honors (possibly
coarse, documented) or fails fast. Perf knobs never silently mislead; security
knobs never silently downgrade.

## Bucket B — required portable

| Knob | WinHTTP mechanism | Verdict | Notes |
| --- | --- | --- | --- |
| `connect_timeout` | `WinHttpSetTimeouts` (resolve+connect) | **Honor** | send/receive left permissive; the pipeline owns end-to-end deadlines. |
| HTTP-version preference | `WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL` (H2/H3 flags) | **Honor (coarse)** | Enables H2 / H3 (H3 on Win11 / Server 2022+) — an HTTP/3 capability hyper cannot offer. H1 cannot be *disabled*, so strict "forbid H1" is not enforceable; documented. |
| mTLS `client_identity` | `WINHTTP_OPTION_CLIENT_CERT_CONTEXT` (+ `ENABLE_HTTP2_PLUS_CLIENT_CERT` for H2) | **Honor** / else **fail fast** | Convert the backend-agnostic DER identity into a Windows `CERT_CONTEXT` (temporary in-memory store). Main TLS implementation effort. |
| `max_connections_per_host` | `WINHTTP_OPTION_MAX_CONNS_PER_SERVER` | **Honor** | Default `INFINITE`; setting `0` caps at 2 (so we never forward a literal 0). |
| streaming bodies / `Extensions` passthrough | `WinHttpReadData` / `WinHttpWriteData`; we read the extensions we understand | **Honor** | See `async-bridge.md`. |
| cancellation on future-drop | `WinHttpCloseHandle` → terminal callback | **Honor** | See the async-bridge cancellation section. |

## Bucket B2 — optional portable (capability-gated)

| Knob | Semantics | WinHTTP mechanism | Verdict |
| --- | --- | --- | --- |
| certificate pinning / validation policy | security | inspect `WINHTTP_OPTION_SERVER_CERT_CONTEXT` in the status callback, atop OS validation | **Honor**, else **fail fast** |
| connection idle timeout | perf | built-in ~60s scavenger; shorten via `WINHTTP_OPTION_EXPIRE_CONNECTION`; prolong (H2) via `WINHTTP_OPTION_HTTP2_KEEPALIVE` | **Honor** (H2 fully; H1 prolong not native — documented) |
| connection max-lifetime | perf | `WINHTTP_OPTION_EXPIRE_CONNECTION` (per-request-handle) | **Honor** (native) |
| coarse keep-alive (on/off + idle health check) | perf | `WINHTTP_DISABLE_KEEP_ALIVE`; idle PING via `WINHTTP_OPTION_HTTP2_KEEPALIVE` (≥5s) | **Honor (coarse)** — fetch's per-connection PING interval/ACK-timeout and active-only mode stay Bucket C |

Notes:

- **Idle timeout** is far less of a gap than earlier drafts assumed. WinHTTP's
  ~60s scavenger already matches fetch-over-hyper's 60s default, so the default
  needs no emulation; shortening is exact (`EXPIRE_CONNECTION`), and prolonging an
  **HTTP/2** connection is native (`HTTP2_KEEPALIVE`). Only prolonging an
  **HTTP/1.1** connection lacks a native mechanism — and TLS session resumption
  makes that rarely necessary. See `winhttp-capabilities.md`.
- **Max-lifetime** is native via `EXPIRE_CONNECTION` (retire the connection after
  the current request completes), giving the same practical outcome as hyper's
  connection poisoning — no session-recycling emulation needed.

## Bucket C — transport-specific (never handed to WinHTTP)

These are hyper-owned and live on `HyperTransport`'s builder, so WinHTTP never
sees them (and a WinHTTP client never exposes them):

- keep-alive PING **timings** (`interval` / ACK `timeout` / active-only) —
  hyper-only granularity.
- HTTP/2 stream tuning (`initial_max_send_streams`, `adaptive_window`) — WinHTTP
  manages H2 flow control internally (`WINHTTP_OPTION_HTTP2_RECEIVE_WINDOW` is only
  a partial analogue, itself a WinHTTP-specific Bucket-C knob).
- connection-pool poisoning internals — a userspace-pool concept.
- userspace TLS backend selection (`Rustls` / `NativeTls` / `PreConfigured`) and
  custom verifiers beyond the portable pinning hook.

WinHTTP's own Bucket-C knobs (proxy/WPAD, integrated Windows auth, SChannel
options) live on `WinHttpOptions` on the WinHTTP transport builder.

## TLS specifics

WinHTTP terminates TLS with **SChannel** against the **Windows certificate
stores**; there is no userspace `ClientConfig` or verifier hook. So:

- **Backend selection** (`Rustls` / `NativeTls` / `PreConfigured`) is Bucket C
  (hyper). It never reaches WinHTTP, so there is nothing to "reject" at the
  WinHTTP boundary — a WinHTTP client is simply never handed a userspace backend.
- **mTLS identity** and **HTTP-version/ALPN preference** are Bucket B (above).
- **Pinning / server-cert validation policy** is Bucket B2 via the server-cert
  callback (above).
- **Custom userspace verifiers** beyond pinning are Bucket C (hyper); a caller
  needing one must use `fetch_hyper`.
- **Revocation** is an always-on invariant, not a knob: WinHTTP sets
  `WINHTTP_ENABLE_SSL_REVOCATION` unconditionally.

### M365 wrinkle — resolved

`fetch_m365` today builds a `rustls` backend wired to `oxidizer_security`'s
certificate `Validator` (SymCrypt/FIPS). **Decision:** M365 policy accepts
WinHTTP's OS-native (SChannel) trust-chain validation in place of the
SymCrypt-backed `Validator`. `builder_m365_winhttp` therefore does **not** wire up
the `Validator`; SChannel validates against the Windows trust stores (with
revocation always on). The `Validator` remains the path only for the hyper/rustls
transports, where TLS is terminated in userspace. This unblocks
`builder_m365_winhttp`.

## Residual gaps (honestly stated)

After the audit, the real residual gaps are small:

1. **Strict "HTTP/1.1 forbidden".** WinHTTP can enable H2/H3 but cannot disable
   H1, so a hyper-style `http2_only` guarantee is not enforceable — Honor (coarse).
2. **Prolonging an idle HTTP/1.1 connection** beyond ~60s has no native mechanism
   (H2 is fine via keep-alive PINGs). Mitigated by TLS session resumption; a
   synthetic warmer is the only workaround and is discouraged.
3. **Idle-timeout granularity.** WinHTTP's scavenger and our `EXPIRE_CONNECTION`
   levers operate per connection but the pool is session-scoped, so very
   fine-grained per-socket idle control is coarser than hyper's. Acceptable.

Everything else the earlier drafts flagged (max-lifetime, idle shortening, keep-
alive, per-server cap, connection targeting) has a native WinHTTP mechanism.

## Decisions still open

- **WinHTTP-native `WinHttpOptions` surface (Bucket C) for phase 1.** Which native
  strengths to expose first (proxy/WPAD, integrated Windows auth, connection GUID
  targeting) vs. defer.
- FFI approach: **decided — `windows-sys`** (see
  [`architecture.md`](architecture.md#unsafe--ffi-policy)).
