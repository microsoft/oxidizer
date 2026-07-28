<!-- Copyright (c) Microsoft Corporation. Licensed under the MIT License. -->

# Async bridge: WinHTTP callbacks → Rust futures

`fetch`'s transport contract is `async`: `execute(&self, HttpRequest) -> impl
Future<Output = Result<HttpResponse>> + Send`. WinHTTP does not natively speak
Rust futures, so the core design question is how to drive WinHTTP without
blocking a runtime worker and without assuming a particular runtime (the
transport must work under both Tokio and the Oxidizer thread-per-core runtime).

## Options considered

### A. Blocking calls on a thread pool (rejected as the primary model)

Use synchronous WinHTTP and offload each request to a blocking thread (Tokio's
`spawn_blocking` or an Oxidizer equivalent). Simple, but:

- Requires a runtime-specific offload primitive → not runtime-agnostic.
- One OS thread parked per in-flight request → poor scaling, defeating the point
  of an async client.
- Streaming request/response bodies map awkwardly onto a single blocking call.

Kept only as a possible fallback for environments where async WinHTTP is
unavailable.

### B. Asynchronous WinHTTP with a status callback (chosen)

Open the session with `WINHTTP_FLAG_ASYNC` and register a status callback via
`WinHttpSetStatusCallback`. WinHTTP performs I/O on its own internal threads and
invokes the callback on completion of each stage
(`SENDREQUEST_COMPLETE`, `HEADERS_AVAILABLE`, `READ_COMPLETE`,
`WRITE_COMPLETE`, `REQUEST_ERROR`, ...). We translate each awaited stage into a
future completion.

This is runtime-agnostic (no executor needed — WinHTTP owns the I/O threads) and
scales without parking a thread per request, which is why the builder needs no
`Spawner`, unlike `fetch_hyper`.

## How the bridge works (option B)

Each request drives a small state machine. At every stage that we `await`, we:

1. Create a `oneshot`-style completion slot (sender + receiver). A minimal
   internal `Waker`-based one-shot avoids pulling in a runtime; the receiver is
   the awaitable.
2. Store the sender where the status callback can reach it — via the
   per-request context pointer WinHTTP threads through
   (`WINHTTP_OPTION_CONTEXT_VALUE`, delivered back as the `dwContext`
   parameter of the callback).
3. Call the WinHTTP operation (`WinHttpSendRequest`, `WinHttpReceiveResponse`,
   `WinHttpReadData`, ...), which returns immediately.
4. `await` the receiver. The callback fires on a WinHTTP thread, fills the slot
   with the result (bytes read / headers ready / error), and wakes the task.

```
task (any runtime)                 WinHTTP internal thread
──────────────────                 ───────────────────────
send_request().await
  set context = &Shared            
  WinHttpSendRequest(...)  ───────► performs TLS + send
  await completion.recv            ...
                          ◄──────── callback: SENDREQUEST_COMPLETE
                                    completion.send(Ok(()))  + wake
  (resumes)
receive_response().await   ───────► ...
  ...
read_body_chunk().await    ───────► READ_COMPLETE → bytes
```

### The context-pointer safety boundary

The `dwContext` value is a raw pointer WinHTTP hands back to the callback on
another thread. This is the crate's central `unsafe` invariant and must be
airtight:

- The pointer targets a pinned, reference-counted per-request state,
  `RequestShared`, kept alive for the whole request and only dropped after the
  terminal callback (`REQUEST_ERROR` or the final `READ_COMPLETE` with zero
  bytes / handle-closing) has run.
- `RequestShared` owns the completion slot(s) and is `Send + Sync`; the callback
  only ever touches it through synchronized fields.
- Handle teardown order is strict: the status callback is cleared (or the handle
  closed, which flushes callbacks) **before** the reference backing `dwContext`
  is allowed to drop, so the callback can never observe a dangling context.

All of this lives in `src/async_bridge.rs` + `src/ffi/`, with `// SAFETY:`
comments on every block, and nothing leaks into the safe surface of the crate.

### Allocating `RequestShared` from a `plurality` pool

`RequestShared` is allocated once per request and freed when the request
finishes — high-churn, single-type, needs a stable address (its pointer is the
`dwContext`) and shared ownership (task side + callback thread). That is exactly
what [`plurality`](../../../plurality) is built for, so we allocate it from a
`plurality::Pool<RequestShared>` and hold it as a `plurality::Arc<RequestShared>`
instead of a global-allocator `std::sync::Arc`. This avoids a `malloc`/`free`
per request: the refcount lives inside the pool's storage, so cloning the handle
for the callback side costs no extra allocation, and the steady-state
allocate/free path is a couple of pointer ops.

Three `plurality` properties make it sound for the FFI bridge specifically:

- **Stable address.** A pooled value never moves while a handle is alive, and
  `plurality::Arc::as_ptr(&shared)` exposes that stable `*const RequestShared`
  directly — this is what we pass as `dwContext`. We keep the owning `Arc` in the
  request handle and pass a *borrowed* raw pointer (no `into_raw`/`from_raw`
  needed); the teardown-ordering rule above already guarantees the owner outlives
  the last callback.
- **Cross-thread frees.** `plurality`'s `Arc` is `Send + Sync` and may be dropped
  from any thread, so the WinHTTP callback thread holding the last reference can
  safely drop it and return the slot to the pool — required by the
  cancellation/teardown path.
- **Bounded growth.** A `max_chunks` cap turns exhaustion into a graceful
  `try_alloc` failure rather than unbounded heap growth.

**Pool ownership vs. `Isolation`.** A `plurality::Pool` is `Send + !Sync`:
allocation takes `&Pool` from a single thread (only the already-allocated
handles are cross-thread). This maps onto `fetch`'s `Isolation`:

- `Isolation::Isolated` (the natural choice under the Oxidizer thread-per-core
  runtime) gives one pool per core with zero cross-thread allocation contention —
  the intended configuration.
- `Isolation::Shared` across a multi-threaded runtime (e.g. Tokio) cannot share a
  single pool for *allocation* without synchronization; that case needs a
  per-thread/per-core pool or a synchronized front. This is a deliberate
  constraint to document on the WinHTTP constructors, not a blocker.

**Pool lifetime and shutdown with in-flight requests.** The `Pool` handle lives
inside the `WinHttpTransport` leaf, so it tracks the transport's lifetime — as
long as the `HttpClient` (a per-core pool under `Isolated`). Crucially, the
*backing storage* is not bounded by the `Pool` handle alone: `plurality`
refcounts the shared pool state, with one reference held by the `Pool` handle and
one by each live `Arc`. The chunks are freed only when that count reaches zero,
so an `Arc<RequestShared>` **outlives the pool handle** if the client is dropped
while a request is still in flight. The `PoolInner` and that request's slot stay
alive until the straggler's terminal callback releases the last `Arc`, keeping
the `dwContext` pointer valid — no use-after-free at shutdown; memory is
reclaimed when the last request finishes. (The only thing lost once the handle is
gone is the ability to *allocate* new slots, which never happens after request
start, when the handler is guaranteed alive.)

(`multitude`'s arena is the wrong tool here: it targets phase-oriented,
bulk-lifetime allocations freed together, whereas per-request state at high churn
over a long-lived transport wants a recycling pool.)

## Cancellation

If the awaiting future is dropped (client-side timeout in the `seatbelt` layers,
task cancellation), the request handle must be closed
(`WinHttpCloseHandle`), which causes WinHTTP to stop and deliver a terminal
`HANDLE_CLOSING`/`REQUEST_ERROR` callback. The RAII handle wrapper's `Drop`
initiates this and defers freeing the `dwContext` state until the terminal
callback has been observed, so cancellation cannot race the callback into
use-after-free.

Because `fetch`'s resilience layers (timeout, retry, hedging) live *above* the
transport, they already drive cancellation by dropping the future — the bridge
just has to make drop safe and prompt.

## Streaming bodies

- **Response**: after `HEADERS_AVAILABLE`, pump `WinHttpReadData` chunk by chunk,
  each read a bridged await, wrapping received bytes in `bytesbuf::BytesView`
  drawn from the `HttpBodyBuilder` so accounting matches other transports. The
  response is returned to the pipeline as soon as headers are available; the body
  streams lazily, matching `fetch_hyper` semantics (and honoring `BodyTimeout`
  from the request extensions).
- **Request**: for non-empty request bodies, feed WinHTTP via `WinHttpWriteData`,
  each write a bridged await. Known-length bodies set `Content-Length`;
  streaming bodies use chunked transfer.

## HTTP/2 and HTTP/3

WinHTTP negotiates HTTP/2 via `WINHTTP_PROTOCOL_FLAG_HTTP2` and HTTP/3 via
`WINHTTP_PROTOCOL_FLAG_HTTP3` (`WinHttpSetOption` on the session; HTTP/2 on
Windows 10 1607+, HTTP/3 on Windows 11 / Server 2022+). The async model is
identical for all three versions; multiplexing (and, for HTTP/3, QUIC transport)
is internal to WinHTTP. Version preference from
`TransportOptions::supported_http_versions` selects which flags are set — HTTP/3
is the capability the default `fetch_hyper` transport cannot offer. See
[`configuration-mapping.md`](configuration-mapping.md).

## Why this keeps the UX transparent

The bridge is entirely internal. The transport still presents the exact same
`Service<HttpRequest, Result<HttpResponse>>` future as every other transport, so
the pipeline above it — and therefore the caller — cannot tell WinHTTP apart
from hyper except through configuration and telemetry attributes.
