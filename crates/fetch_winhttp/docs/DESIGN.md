# `fetch_winhttp` design

Status: design, pre-implementation. This document describes the architecture of
the `fetch_winhttp` crate. The crate currently ships only this design and a
placeholder `lib.rs`.

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
  v1; see §9.1.)
- **OS-managed protocol stack.** HTTP/1.1, HTTP/2 and HTTP/3 negotiation,
  connection pooling, keep-alive, proxy discovery and automatic gzip/deflate
  decompression are handled by the OS.
- **Smaller dependency surface.** No rustls/aws-lc-rs/native-tls/hyper on the
  request path.

Out of scope: any non-Windows platform (the crate is `#[cfg(windows)]` in its
entirety); WebSocket upgrades; proxy auto-config scripting beyond what WinHTTP
does natively.

### 1.1 Where it plugs into `fetch`

`fetch` exposes a runtime-agnostic custom-transport extension point. A transport
is any type implementing
`layered::Service<HttpRequest, Out = fetch::Result<HttpResponse>>` (the
[`RequestHandler`] trait alias), constructed through
`fetch::custom::create_builder`:

```rust,ignore
pub fn create_builder<F, R, Extras>(
    runtime: impl Into<Cow<'static, str>>,   // telemetry "fetch.runtime"
    transport: impl Into<Cow<'static, str>>, // telemetry "fetch.transport"
    factory: F,                              // Fn(CustomContext<Extras>) -> R
    isolation: Isolation,
    deps: impl Into<CustomDeps<Extras>>,
) -> HttpClientBuilder
where
    F: Fn(CustomContext<Extras>) -> R + Send + Sync + 'static,
    R: RequestHandler + 'static,
    Extras: ThreadAware + Send + Sync + Clone + 'static;
```

`CustomContext` hands the factory a `HttpBodyBuilder` (carrying the clock and
read-buffer pool), a `PoolIndex`, the generic `TransportOptions`/`TlsOptions`, a
`Meter`, and the caller's `Extras`. `fetch_winhttp` ignores `PoolIndex` (per-core
placement comes from `Isolation::Isolated`, §4.2) and, like `fetch_hyper`, layers on
this same extension point rather than a bespoke one.

`fetch_winhttp`'s public entry point is a pair of free functions in the crate root:

```rust,ignore
#[derive(thread_aware::ThreadAware)]
pub struct WinHttpDeps {
    pub tls: WinHttpTlsConfig,     // WinHTTP-specific TLS knobs (§9)
    pub options: WinHttpOptions,   // WinHTTP-specific tuning (§8, §10, §12)
}

/// Creates an `HttpClientBuilder` wired to the WinHTTP transport.
pub fn builder(deps: impl Into<WinHttpDeps>) -> HttpClientBuilder;
/// Builds an `HttpClient` on the WinHTTP transport with default deps.
pub fn new() -> HttpClient;
```

They are free functions, not methods on `HttpClient`, because `HttpClient` belongs
to `fetch` and the orphan rule forbids an inherent `impl` here (a `fetch` API wart;
§17).

`builder` opens the one shared OS session (§4.2) up front, then calls
`create_builder("winhttp", "winhttp", factory, Isolation::Isolated, deps)`. The
session is deliberately **not** a `WinHttpDeps` field: the factory closure captures
it and `Arc`-clones it into each per-core transport instance it builds, keeping
`WinHttpDeps` to plain, relocatable configuration. `WinHttpDeps` derives
`ThreadAware` so `fetch` can clone and relocate that config per core (§4.2). The
clock and read-buffer pool come from `CustomContext`, so they are not duplicated in
`Extras`. There is no `anyspawn::Spawner`: no WinHTTP call the transport makes can
block (§3.1).

### 1.2 TLS is configured on the transport, not through `fetch`'s `TlsOptions`

`fetch`'s generic `TlsOptions`/`TlsBackend` carries rustls/native-tls material
(crypto providers, verifiers, client-cert resolvers) that is meaningless to
Schannel. WinHTTP does TLS itself and accepts only a small set of knobs, so
`fetch_winhttp` ignores `CustomContext::tls` and takes its own `WinHttpTlsConfig`
through `Extras` (§9). Different transports inherently support different TLS
configuration models, so trying to configure TLS uniformly at the transport-abstract
`fetch` level is over-abstraction on `fetch`'s part; see §17.

### 1.3 Architecture at a glance

The whole design in one picture, so later sections can be read as elaborations of
this model rather than assembled from details:

- **One shared OS session per built client**, opened at build time, `Arc`-shared by
  all per-core instances, and immutable after setup (§4.2).
- **One transport instance per core.** `fetch` clones and relocates the transport
  per core (`Isolation::Isolated`, §4.2); each instance owns its object and event
  pools (§6) and holds a clone of the one shared session `Arc`.
- **One `RequestDriver` future per request** (§5.4). It owns a `RequestGuard`
  bundling the request handle, its connect handle, and a session-`Arc` clone, and
  rents a pooled `RequestContext` - the small slot WinHTTP calls back into (§5.1).
- **WinHTTP drives the I/O on its own threads** (§4). The transport issues
  asynchronous calls and each one signals completion back to the awaiting future
  through an `events_once` one-shot (§4.3). A completion runs either inline on the
  submitting thread (keeping work on one processor) or on a WinHTTP worker thread.
- **No blocking pool and no Tokio.** Every setup call is synchronous but performs no
  I/O, so it runs inline on the executor; only WinHTTP's own async steps defer (§3.1).
- **Ownership across FFI is callback-driven** (§5). Dropping the guard closes the
  handles synchronously, but the `RequestContext` is freed only on WinHTTP's final
  `HANDLE_CLOSING` callback, which guarantees no use-after-free under cancellation.

## 2. The bindings facade (OS abstraction for testability)

Following the reference pattern in `oxidizer_io`
(`ox-sdk/crates/oxidizer_io/src/pal/windows/bindings/*`), every WinHTTP OS call
goes through a `Bindings` trait, never a direct `windows`-crate call from
business logic. This is the single most important structural decision because it
is what makes the transport unit-testable without a network or even a real OS
handle.

```rust,ignore
/// Every WinHTTP OS entry point the transport uses, and nothing else.
#[cfg_attr(test, mockall::automock)]
pub(crate) trait Bindings: Send + Sync + 'static {
    fn open(&self, flags: u32 /* WINHTTP_FLAG_ASYNC */) -> Result<SessionHandle>;
    fn set_timeouts(&self, h: RawHandle, resolve: i32, connect: i32, send: i32, receive: i32) -> Result<()>;
    fn set_status_callback(&self, h: RawHandle, cb: WINHTTP_STATUS_CALLBACK, flags: u32) -> Result<()>;
    fn connect(&self, session: RawHandle, host: &U16CStr, port: u16) -> Result<ConnectHandle>;
    fn open_request(&self, connect: RawHandle, method: &U16CStr, path: &U16CStr, secure: bool) -> Result<RequestHandle>;
    fn set_option_u32(&self, h: RawHandle, option: u32, value: u32) -> Result<()>;
    fn set_option_bytes(&self, h: RawHandle, option: u32, value: &[u8]) -> Result<()>;
    fn set_context(&self, h: RawHandle, ctx: usize) -> Result<()>;  // WINHTTP_OPTION_CONTEXT_VALUE
    fn send_request(&self, h: RawHandle, headers: &U16CStr, optional: Option<&[u8]>, total_len: u32) -> Result<()>;
    fn write_data(&self, h: RawHandle, buf: *const u8, len: u32) -> Result<()>;
    fn receive_response(&self, h: RawHandle) -> Result<()>;
    fn query_headers_raw(&self, h: RawHandle) -> Result<Vec<u16>>;   // WINHTTP_QUERY_RAW_HEADERS_CRLF
    fn query_status_code(&self, h: RawHandle) -> Result<u32>;
    fn query_protocol_used(&self, h: RawHandle) -> Result<http::Version>; // WINHTTP_OPTION_HTTP_PROTOCOL_USED
    fn query_data_available(&self, h: RawHandle) -> Result<()>;      // async -> DATA_AVAILABLE
    fn read_data(&self, h: RawHandle, buf: *mut u8, len: u32) -> Result<()>; // async -> READ_COMPLETE
    fn close_handle(&self, h: RawHandle);
}
```

- **Production impl** (`RealBindings`) wraps the `windows`-crate calls one-to-one
  with `// SAFETY:` notes, like `oxidizer_io`'s build-target bindings. Every
  referenced symbol exists in `windows` `0.62.2`.
- **Test impl** is `mockall`'s generated `MockBindings`, wrapped in a `Facade`
  enum (`Real(&'static RealBindings)` / `Mock(Arc<MockBindings>)`), matching
  `oxidizer_io`'s bindings facade.
- The status callback cannot itself be a trait method (WinHTTP calls a bare
  `extern "system"` fn pointer). Tests therefore synthesize callbacks by invoking
  the crate-internal `dispatch_completion(context, status, info)` directly. That
  entry point is a plain fn precisely because it needs no captured state: all
  per-request state is reached through the `context` pointer (the `*mut
  RequestContext`, §5.2), and all recording/expectation state lives in the
  `Arc<MockBindings>` the harness owns via the `Facade`. Nothing is a global
  singleton. See §14.

**Safety contract of the `Bindings` API.** Because callers drive raw OS handles and
async buffer lifetimes through this trait, a small set of caller-side invariants
must hold for every impl (production or mock) to be sound. They are stated once here
and relied on throughout §5 and §11:

- A buffer handed to `write_data`/`read_data` must stay valid and untouched until
  that operation's completion callback fires (WinHTTP borrows it asynchronously).
- The `RequestContext` must be fully populated and every borrow of it dropped
  **before** the async call is issued, so the completion (possibly reentrant, §3.1)
  has exclusive access via the context pointer.
- At most one async operation is outstanding per request handle at a time.
- The status callback must be registered (with the handle-close flag) and the
  context installed before the first async call, and each handle is closed exactly
  once (§5.3).

### 2.1 Crate/module layout

```text
crates/fetch_winhttp/
  src/
    lib.rs               // #![cfg(windows)] gate + re-exports + crate docs
    builder.rs           // WinHttpDeps, WinHttpOptions, builder()/new()
    transport.rs         // WinHttpTransport: per-core Service<HttpRequest> (§4.2)
    session.rs           // shared session handle (one per transport)
    request.rs           // RequestDriver: one request/response lifecycle
    context.rs           // RequestContext (per-operation FFI context; pooled)
    callback.rs          // extern "system" trampoline -> dispatch_completion
    body/
      read.rs            // bytesbuf_io::Read over WinHttpReadData (response)
      write.rs           // bytesbuf_io::Write over WinHttpWriteData (request)
    tls.rs               // WinHttpTlsConfig -> security flags
    options.rs           // protocol/decompression option mapping
    handle.rs            // RAII handle wrappers (Send/Sync assertions)
    error.rs             // Win32 -> HttpError mapping
    error_labels.rs      // ErrorLabel constants
    bindings/
      abstractions.rs    // Bindings trait (OS entry-point contract)
      facade.rs          // Facade enum (Real / Mock dispatch)
      real.rs            // windows-crate impl (cfg(windows))
      mod.rs             // module wiring only (no type definitions)
  docs/DESIGN.md
```

Because the crate root is gated by `#![cfg(windows)]`, it compiles to an empty
module on non-Windows targets. On non-Windows CI legs the crate therefore builds to
nothing, pulling in no `windows` dependency, so `members = ["crates/*"]` stays green.

## 3. WinHTTP asynchronous model primer

A single request drives this WinHTTP handle chain and callback sequence:

| Step | Call | Sync/async | Completion callback |
|------|------|-----------|---------------------|
| S1 | `WinHttpOpen(WINHTTP_FLAG_ASYNC)` | sync | - (build-time) |
| S2 | `WinHttpSetStatusCallback` (session-level; inherited by all requests) | sync | - (build-time; mask in §5.3) |
| 3 | `WinHttpConnect` | sync, inline (see §3.1) | - |
| 4 | `WinHttpOpenRequest` | sync | - |
| 5 | `WinHttpSetOption`xN (incl. context), `WinHttpSetTimeouts` | sync | - |
| 6 | `WinHttpSendRequest` | async | `SENDREQUEST_COMPLETE` |
| 6a| `WinHttpWriteData` (streaming body, per chunk) | async | `WRITE_COMPLETE` |
| 7 | `WinHttpReceiveResponse` | async | `HEADERS_AVAILABLE` |
| 8 | `WinHttpQueryHeaders` | sync (buffered) | - |
| 9 | `WinHttpQueryDataAvailable` | async | `DATA_AVAILABLE` (n bytes) |
| 10| `WinHttpReadData` | async | `READ_COMPLETE` (n bytes) then loop 9/10 until 0 |
| 11| `WinHttpCloseHandle` | sync | `HANDLE_CLOSING` (final callback) |

Errors on any async step arrive as `REQUEST_ERROR` carrying a
`WINHTTP_ASYNC_RESULT { dwResult, dwError }`. TLS validation problems also raise
`SECURE_FAILURE` before the `REQUEST_ERROR`.

### 3.1 Synchronous setup calls run inline (no blocking pool)

"Synchronous" in WinHTTP means "returns its result directly rather than via a
completion callback" - not "blocks on I/O". The setup calls the transport makes
(`WinHttpConnect`, `WinHttpOpenRequest`, the `WinHttpSetOption`/`WinHttpSetTimeouts`
family, `WinHttpQueryHeaders`) do no network, DNS, or socket work; they allocate and
configure handles, deferring all I/O to the async steps. They are therefore safe to
run inline on an executor thread, and the transport needs **no** `anyspawn::Spawner`
and no blocking pool.

The sole exception is the very first `WinHttpOpen` in a process, which runs
WinHTTP's one-time global initialization (a lock plus registry reads) and can
briefly block. Since the transport opens its single session once at build time
(§4.2), this is a one-time construction cost off the request path.

The session sets `WINHTTP_OPTION_ASSURED_NON_BLOCKING_CALLBACKS`: we promise our
completion callbacks never block, and in return WinHTTP may invoke a callback
**inline** on the submitting thread whenever an operation completes immediately
(e.g. a read served from an internal buffer), instead of hopping to a thread-pool
worker. We want this - it removes a thread-pool hop on the hot path (§4.1).

The callback trampoline (§5) is safe to run reentrantly because it does a small,
bounded, non-blocking amount of work: recover the `*mut RequestContext`, take the
in-flight `events_once` sender and buffer, and send the `CompletionResult`. It
performs no I/O and never waits on WinHTTP. Returning pooled memory (an
`events_once` endpoint, the context `Box`, a `BytesBuf`) on a cancellation or
`HANDLE_CLOSING` path is likewise non-blocking. The one heavier case - the last
context `Box` drop freeing the `plurality` pool's chunks - happens only at shutdown,
where cost is irrelevant and only correctness matters; it is a heap free, not a wait
on WinHTTP, so the assurance still holds.

Reentrancy is sound because of one submitting-side rule (§5.5): the driver fully
populates the `RequestContext` and drops every borrow to it **before** issuing the
async call. However the completion then arrives, it has exclusive access through the
leaked pointer, and the `events_once` send is the single release/acquire edge that
hands buffer ownership back.

## 4. Threading model

Two kinds of threads are in play, and the design is about moving data safely
between them:

1. **Async executor threads** (the caller's runtime). `WinHttpTransport::execute`
   runs here and the returned future is polled here. Every setup call (§3.1) runs
   inline on these threads; `fetch` requires the returned future to be `Send`.
2. **WinHTTP's own worker threads.** When a completion cannot be produced
   immediately, WinHTTP delivers it later on a thread it owns. The application
   neither creates, sizes, nor owns these threads.

Because we set `ASSURED_NON_BLOCKING_CALLBACKS` (§3.1), a completion that *can* be
produced immediately is delivered **inline on the executor thread that submitted the
call**, reentrantly; otherwise it arrives later on a WinHTTP worker. The callback
trampoline (§5) is identical either way, and there is no third "blocking pool" tier -
no request-path call can block (§3.1).

### 4.1 How WinHTTP schedules callback work

WinHTTP does not run a bespoke thread pool; it posts async completions to the
process-global Win32 thread pool, from which a worker dispatches our callback. There
is no per-request or per-handle thread affinity: successive completions for one
request can land on different workers, and we do **not** assume WinHTTP serializes
callbacks per handle. Soundness rests only on "exactly one completion per async
operation" plus "one operation outstanding per handle" (§5.5), with the single
status-vs-completion race closed by an atomic (§5.5).

Two consequences shape the design: all per-request callback state must be reachable
from the context pointer alone and safe to touch from any thread (§5.1, with `Send`
handle wrappers, §4.4); and the callback-to-future handoff must be a real
cross-thread signal, not a shared cell - that is `events_once` (§4.3), which behaves
identically same-thread or cross-thread.

Inline completions (§3.1) give a degree of processor affinity for free: when an
operation completes immediately, WinHTTP runs the callback on the very thread that
submitted it, so completion work stays on the same processor as the async work that
issued it rather than hopping to an arbitrary thread-pool worker. Only genuinely
deferred completions incur the hop.

### 4.2 Per-core transport instances and the one shared session

`fetch_winhttp` registers with `Isolation::Isolated`. Under `Isolated`, `fetch`
stores the *config plus a factory* and, the first time each core touches it, clones
the config, relocates it to that core, and runs the factory to build a fresh
transport instance for that core (cached per affinity). So each core gets its own
newly built `WinHttpTransport` with core-local pools (§6). (`Isolation::Shared` would
instead build one instance and share it across all cores.) We choose `Isolated` so
the `!Sync` `plurality` object pool (§6) can be core-local; the handler must still be
`Sync`, so that one pool sits behind a coarse `Mutex` (§6). `WinHttpDeps` derives
`ThreadAware` so `fetch` can clone and relocate the config per core.

The one piece of state that is **not** rebuilt per core is the OS session. A session
owns session-scoped state - most importantly the connection (keep-alive) pool - so a
single session gives one shared connection pool across all cores, and the
process-global callback thread pool (§4.1) means extra sessions would add duplicated
state without adding throughput. The session is opened once at build time, wrapped in
`Arc<WinHttpSession>`, captured by the factory closure, and `Arc`-cloned into every
per-core instance (§1.1). It is immutable after setup, so a plain `Arc` suffices; the
per-core object pool is the only mutable shared state (`Mutex`-guarded), while the
event pool and read-buffer `GlobalPool` are already thread-safe. All are uncontended
under thread-per-core use.

**Contrast with `fetch_hyper`.** `fetch_hyper` uses `Isolation::Shared`: one hyper
client, already fully thread-safe, shared across cores, so its pool is process-wide
by construction. `fetch_winhttp` reaches the same end state (one connection pool per
client) from the other direction - per-core instances sharing only the session that
owns the pool.

**Future exploration.** Two choices here are conservative defaults, open to revision
after performance analysis: (a) a *single* session per transport - a future
multi-session design could recycle connections at session granularity, supplying the
connection-lifetime control WinHTTP otherwise denies us (§7.5); and (b) *cross-core*
connection reuse via one shared session - it is not obvious this beats per-core
sessions with warmer, core-local pools, so v2 may revisit it.

Connection management (connect handles, reuse, lifetime) gets its own chapter (§7).

### 4.3 Callback to future handoff via `events_once`

Each async WinHTTP step is a one-time signal from the callback (whether it fires
inline on the submitting thread or later on a WinHTTP worker thread, §4.1) to one
awaiting future, carrying a small payload. That is exactly `events_once`.

For each async step the `RequestDriver` (§5.4) rents a
`(sender, receiver)` pair from a transport-owned, per-core
`events_once::EventPool<CompletionResult>` (§6), stores the sender in the
request's `RequestContext`, issues the async call through `Bindings`, and awaits
the receiver. When WinHTTP later invokes the callback trampoline, the trampoline
reconstructs the `RequestContext` from the context value, takes the stored
sender, builds a `CompletionResult`, and sends it. The executor wakes and the
driver advances to the next state.

```rust,ignore
enum CompletionResult {
    SendComplete,
    WriteComplete,
    HeadersAvailable,
    DataAvailable(u32),
    // Ownership of the read buffer is returned to the future here. `len` is the
    // number of bytes WinHTTP appended (metadata; the buffer may have carried
    // earlier bytes, since a BytesBuf need not be empty to be appended to).
    ReadComplete { buffer: bytesbuf::BytesBuf, len: u32 },
    Error(HttpError),
}
```

`events_once` is the right primitive because each step is a single, non-blocking,
one-shot, payload-carrying signal with exactly one waiter.

### 4.4 `Send` (not `Sync`) across the FFI boundary

Raw WinHTTP handles are `*mut c_void` and thus neither `Send` nor `Sync`. They
are wrapped in `handle.rs` newtypes with explicit unsafe marker impls justified by
WinHTTP's documented cross-thread handle usability, mirroring the
`ThreadSafe<HANDLE>` technique in `oxidizer_io`. Two tiers, because their sharing
needs differ:

- **Request and connect handles are `Send` but not `Sync`.** Each belongs to one
  request; the handle is only ever *moved* between threads (the future migrates
  across executor threads, and a completion may arrive on a different thread than
  the submit), never shared by reference from two threads at once. The driver keeps
  at most one operation outstanding per handle and holds the only reference, so
  `Send` alone is what we need and all we can honestly assert.
- **The session handle is `Send + Sync`.** It is the one object shared by reference
  across cores (an `Arc<WinHttpSession>` captured by the factory closure, §1.1/§4.2).
  The closure must be `Send + Sync`, so the captured session must be too. This is
  sound because WinHTTP explicitly permits concurrent operations on one session
  handle, and after build-time setup the session is read-only from our side (§4.2);
  the `unsafe impl Sync for WinHttpSession` carries exactly that justification.

The future therefore holds only `Send` state (an `events_once` receiver plus
request/connect handle wrappers and a shared session `Arc`), satisfying `fetch`'s
`Out: Send` requirement.

## 5. Cancellation model and FFI ownership

This is the subtlest part of the design and the most likely source of unsafety if
done naively, so it gets its own chapter.

**The hazard.** For `WinHttpReadData`/`WinHttpWriteData`, the caller-supplied
buffer must remain valid until the corresponding `READ_COMPLETE`/`WRITE_COMPLETE`
callback fires; WinHTTP reads from or writes into that memory asynchronously on
its own thread. Likewise the request context that the callback dereferences must
remain valid until WinHTTP is done with the handle. If a `fetch` caller abandons the
request - dropping the in-flight `execute` future before headers, or the response
body while a read is outstanding (timeout, `select!`, client shutdown) - we must not
free the buffer or the context until WinHTTP promises it is finished.

### 5.1 The per-request operation slot

WinHTTP allows at most one outstanding async operation per request handle at a
time, and it delivers every completion for a handle to the same callback context
pointer. `RequestContext` is therefore really an operation slot: all of its data
is operation-level (the current completion sender and the buffer that operation
borrows), and it holds no request-level state of its own. It exists at request
scope purely so a single allocation is reused across the request's sequence of
sequential operations (send, then receive, then each read) instead of being
reallocated per step. Its pointer is what we hand to WinHTTP as the callback
context; WinHTTP echoes it back on every notification for that request handle.

The request handle lives in the driver (§5.4), not in this context: the callback
only recovers the context, takes the sender and buffer, and signals (§3.1), while
the driver uses the handle to issue the next call and, once, to close. That split
gives a single close authority (the driver's `RequestGuard`).

```rust,ignore
// `Idle` between operations; `Active` for the single in-flight async operation.
// Modeling it as an enum makes the invariant structural: there is no completion
// sender, borrowed buffer, or cert-failure flag unless an operation is running.
enum RequestContext {
    Idle,
    Active {
        // Completion sender for the in-flight operation; the callback takes it.
        completion: events_once::PooledSender<CompletionResult>,
        // The buffer this operation borrows (if any). Read ops borrow a mutable
        // BytesBuf (WinHTTP appends response bytes); write ops borrow an immutable
        // BytesView (WinHTTP reads request bytes); send/receive/query ops borrow
        // none. Ownership passes to WinHTTP for the operation's duration (§5).
        buffer: OperationBuffer,
        // Set by a SECURE_FAILURE status callback. On the send path WinHTTP fires
        // SECURE_FAILURE before the operation's terminal REQUEST_ERROR, so the
        // occurrence order is WinHTTP-guaranteed; the AtomicU32 (vs Cell) only
        // supplies the cross-thread publication edge, since the two callbacks may
        // run on different threads. See §5.5.
        secure_failure_flags: core::sync::atomic::AtomicU32,
    },
}

enum OperationBuffer {
    None,
    Read(bytesbuf::BytesBuf),
    Write(bytesbuf::BytesView),
}
```

The enum makes the field relationships explicit: `Active` always carries a
completion sender, at most one borrowed buffer (a handle never has a read and a
write outstanding at once), and the cert-failure flag; `Idle` carries nothing. The
callback moves `Active -> Idle` by `take`-ing the sender and buffer.

### 5.2 dwContext is pointer-sized

WinHTTP stores the callback context as a `DWORD_PTR`, which is by definition
pointer-sized on Windows. `WINHTTP_OPTION_CONTEXT_VALUE` sets it by reading a
`*(DWORD_PTR*)`, and the callback receives that same value as its `dwContext`
parameter. A raw `*mut RequestContext` therefore round-trips through the context
value without truncation. This is the established production pattern for async
WinHTTP transports (store an owning per-request pointer in the context value and
reclaim it on `HANDLE_CLOSING`); we use the same shape with a pooled context
(§6) rather than an `Arc`.

### 5.3 Ownership rule: the pool owns the context, the callback frees it

A `RequestContext` is a `plurality::Box<RequestContext>` rented from a per-core
pool (§6) and handed to WinHTTP as the opaque handle context. One rule governs its
lifetime: **the driver owns the `Box` until `WinHttpSetOption(CONTEXT_VALUE)`
succeeds; after that WinHTTP owns it and the callback reclaims it on the final
`HANDLE_CLOSING`.**

The status callback is registered once on the session handle at build time (§3, step
S2) with mask `ALL_COMPLETIONS | SECURE_FAILURE | HANDLES`, and every request handle
inherits it. The only per-request handoff is installing the context pointer via
`WinHttpSetOption(WINHTTP_OPTION_CONTEXT_VALUE, ptr)`.

Before that `SetOption` succeeds, a failed `WinHttpOpenRequest` or `SetOption` lets
the driver drop the `Box` directly back into the pool. This is safe because WinHTTP
initializes a handle's context to null, and the trampoline ignores any callback
(including `HANDLE_CLOSING`) whose context is null - so early-failed requests,
connect handles, and the session never reconstruct a `Box`.

After `SetOption` succeeds, dropping the `RequestGuard` owner (the driver or the
`WinHttpBodyReader` it moves into, §11.3) synchronously closes the request and
connect handles and releases the session `Arc`, but does **not** free the context.
Closing the request handle aborts any outstanding operation and makes WinHTTP deliver
one final `HANDLE_CLOSING`, where the trampoline reclaims the `Box`. The context is
thus the single deferred free; releasing the parent handles early is safe because
WinHTTP reference-counts them internally and keeps them alive until the child request
finishes tearing down.

Because the pool's backing memory is reference-counted (like the `Arc`-backed
`events_once` pools), the context stays valid even after the transport or request
future is gone - validity is tied to the callback protocol, not to transport or
request lifetime, the same deferred-free discipline as `oxidizer_io`'s IOCP path.
Reclaiming the `Box` across the FFI boundary uses `plurality::Box::into_raw` /
`from_raw`, so the context pointer both identifies and owns the `RequestContext`
with no side registry.

### 5.4 The request lifecycle is the `RequestDriver`

The "state machine" that issues the calls above is `RequestDriver` in
`request.rs`: the concrete async body that `WinHttpTransport::execute` returns and
polls. It walks the steps of §3, awaiting each step's `events_once` receiver, and
owns the `RequestGuard` - the request handle, its connect handle, a clone of the
shared `Arc<WinHttpSession>`, and the raw `RequestContext` pointer - whose drop
performs the synchronous teardown described in §5.3. When the design refers to "the
driver" it means this type. The handles live here, not in the context (§5.1); the
context is only the completion mailbox.

### 5.5 Exclusive access without locks

The buffers and sender in `RequestContext` are shared between the driver and the
callback with no lock. This is sound because access is strictly non-overlapping in
time, enforced by one discipline:

- **The driver populates the context and drops every borrow to it before issuing
  the async call.** It moves the `RequestContext` to `Active { .. }` (installing the
  completion sender and the operation's buffer) through the raw pointer, ends that
  borrow, *then* calls `Bindings::read_data` (etc.). It holds no
  `&mut RequestContext` across the submit boundary.
- From the submit call until the `events_once` receiver resolves, the driver
  touches nothing in the context. WinHTTP holds exclusive ownership of the leaked
  pointer for the operation's duration (§5.2).
- The **completion** for the operation - inline on the submitting thread, or later
  on a worker thread - is the sole accessor of the `Active` fields: it `take`s the
  sender and buffer (moving the context back to `Idle`) and sends. WinHTTP delivers
  exactly one completion per async operation, and the driver keeps one operation
  outstanding per handle, so no second callback ever touches those fields. This
  exclusivity does **not** rely on any undocumented per-handle callback
  serialization; it follows from "one completion per op" plus "one op outstanding".
- The `events_once` send-then-receive is the release/acquire edge that transfers
  buffer ownership back to the driver. Only after the receiver resolves does the
  driver read the returned buffer.

This is exactly the "WinHTTP takes exclusive ownership via a leaked pointer, we
recover it at the callback" model: the leaked pointer *is* the ownership token,
and the two sides never hold it at the same time. No lock is needed on the
sender/buffer fields; the temporal hand-off does the work.

**The one field that is not covered by the temporal hand-off** is
`secure_failure_flags`, and that is why it is an `AtomicU32` rather than a `Cell`.
On the send path WinHTTP fires the `SECURE_FAILURE` status notification *before* it
generates the operation's terminal `REQUEST_ERROR` completion, so the completion is
certain to observe a cert failure that has already occurred. WinHTTP does not
promise the two callbacks run on the same thread, so the trampoline uses the atomic
purely as a cross-thread publication edge: a `SECURE_FAILURE` status publishes the
certificate-error bitmask with a `Release` store and touches nothing else; the
`REQUEST_ERROR` completion reads it with an `Acquire` load before doing the
sender/buffer hand-off. The atomic supplies the publication edge; the occurrence
ordering that makes the read meaningful is WinHTTP's. Every branch remains
non-blocking (§3.1).

## 6. Object and event pooling

Callbacks are hot and frequent and each request issues several async steps, so
the transport avoids per-step allocation by keeping two transport-owned pools that
requests rent from. Both must fit the `Send + Sync` bound a `fetch` handler
requires: the handler is stored in an `Arc<T>` shared into every request
future, and `Isolation::Isolated` gives each core its own instance but does not
relax that bound.

- **`events_once::EventPool<CompletionResult>`.** For each async step the driver
  (or the response body reader) rents one `(sender, receiver)` pair, and the callback
  sends the completion through it (§4.3). The pool is already `Send + Sync`, so it is
  held as a plain field, cloned into each request, and a callback may complete an
  event on any thread.
- **`plurality::Pool<RequestContext>` behind a `Mutex`.** A request rents one
  `plurality::Box<RequestContext>` at start and holds it for its whole lifetime
  (reclaimed when the callback drops it on `HANDLE_CLOSING`, §5.3). `plurality::Pool`
  is `Send + !Sync`, so it is the one field we wrap in a `std::sync::Mutex`. The lock
  is coarse but essentially uncontended: it is taken only to rent a context at request
  start - never across an `.await`, never by the body reader (which reuses the
  already-rented context), and never in a callback (the context returns itself to the
  pool through its own `Drop`, holding no `&Pool`).

Read buffers come from neither pool: they are reserved from the `bytesbuf`
`GlobalPool` on `CustomContext` (a `Sync`, `Arc`-backed pool), so the body reader
holds its own clone and rents buffers with no lock.

## 7. Connection management

WinHTTP owns the connection pool, so honoring `fetch`'s connection options means
understanding exactly what a connect handle is and what WinHTTP does and does not
let us control.

### 7.1 Connect handles are logical, not connections

`WinHttpConnect` returns a logical `(host, port)` handle. It performs no network
I/O; it validates and normalizes the host name and allocates the handle. The
actual TCP/TLS connections are established later by `WinHttpSendRequest` and are
owned and pooled by the session's WebIO layer, keyed by authority. Consequences:

- A connect handle is an addressing abstraction, not a socket. Closing a connect
  handle destroys only the logical handle; it does not close the pooled transport
  connections underneath (those remain in the session pool). The connect-handle
  destructor in WinHTTP is empty.
- Reuse is automatic and session-managed. Issuing successive requests through the
  same session against the same authority reuses pooled keep-alive connections;
  we do not open or bind connections ourselves.

Because a connect handle is cheap, non-blocking, and carries no connection state,
`fetch_winhttp` opens one per request via `WinHttpConnect` and closes it when the
request ends. There is **no** connect-handle cache. Caching would add shared
mutable state (a per-authority map, which under a shared session would need
synchronization) to save a call that does no I/O, and it would buy no connection
reuse: reuse is keyed by authority in the session's pool and is independent of
which connect handle a request used. Dropping the cache keeps the transport's
shared state limited to the read-only session (§4.2).

### 7.2 HTTP/1.1 serialization and concurrency

For HTTP/1.1 there is no multiplexing: concurrent requests to the same authority
are serviced by separate pooled connections, bounded by
`WINHTTP_OPTION_MAX_CONNS_PER_SERVER`. WinHTTP performs this
serialization/pooling automatically; we do not manually serialize requests onto a
connection. HTTP/2 and HTTP/3 multiplex many requests over a single connection,
also handled by WinHTTP. Our only lever on concurrency is the max-connections
option (§7.4).

### 7.3 Our responsibilities for reuse and draining

To get connection reuse we must: keep one session (§4.2) and not set
`WINHTTP_DISABLE_KEEP_ALIVE`. WinHTTP does the rest - reuse is session-level and
keyed by authority, independent of connect-handle identity (§7.1), so opening a
fresh connect handle per request costs nothing in reuse terms.

**We do set `WINHTTP_OPTION_DISABLE_GLOBAL_POOLING` on the session at creation.**
By default WinHTTP may share pooled connections process-wide across sessions. That
would let two independent `fetch_winhttp` clients - for instance a strict one and
one built with `accept_invalid_certs` (§9) - reuse each other's pooled
connections, collapsing the security boundary between them. Disabling global
pooling scopes the connection pool to this session, so each `HttpClient` gets its
own pool (still one pool per client, shared across that client's cores, §4.2)
while different clients stay isolated. Reuse within a client is unaffected.

For draining, WinHTTP exposes only coarse controls:

- `PurgeKeepAlives` on the session retires idle pooled connections session-wide.
  There is no API to drain one specific connection.
- Per request, `WINHTTP_OPTION_DISABLE_FEATURE | WINHTTP_DISABLE_KEEP_ALIVE`
  prevents that request from keeping or reusing a pooled connection.

Individual pooled connections are opaque to us: WinHTTP exposes no per-connection
handle, age, or close, so a graceful drain of a single connection is not
expressible. The v1 transport keeps exactly one session for its whole lifetime
and does not recycle it. Connection-lifetime handling under that constraint is
covered in §7.5.

### 7.4 Mapping `fetch` connection-pool options onto WinHTTP

`fetch_options::ConnectionPoolOptions` (reached via `TransportOptions`) exposes
`max_connections`, `connection_idle_timeout`, and `connection_lifetime`, plus
`ConnectionKeepAlive`. WinHTTP's controls do not map one-to-one, so the mapping is
partly advisory and is documented as such:

| `fetch` option | WinHTTP mechanism | Fidelity |
|----------------|-------------------|----------|
| `max_connections` (per pool) | `WINHTTP_OPTION_MAX_CONNS_PER_SERVER` | Advisory: WinHTTP bounds per authority, `fetch` counts per pool. |
| `connection_idle_timeout` | WinHTTP's own idle keep-alive management; `PurgeKeepAlives` to force-clear | Advisory: no exact idle-TTL knob is exposed; WinHTTP applies its own default. |
| `connection_lifetime = Unlimited` (default) | nothing to do | Exact. |
| `connection_lifetime = Fixed(_)` / `PerConnection(_)` | not honored in v1 (see §7.5) | Documented limitation. |
| `ConnectionKeepAlive::ActiveConnections{..}` | `WINHTTP_OPTION_HTTP2_KEEPALIVE` / `WINHTTP_OPTION_HTTP3_KEEPALIVE` (floor 5000 ms) | Approximate for h2/h3; HTTP/1.1 keep-alive is automatic. |
| `ConnectionKeepAlive::Disabled` (default) | leave keep-alive at WinHTTP defaults | n/a |

`ConnectionInfo` (age, `is_expired`, poisoning) that `fetch_hyper` attaches to
responses cannot be reproduced: WinHTTP hides individual connections, so per
connection age is not observable and no per-connection identity is exposed.
(`WINHTTP_OPTION_EXPIRE_CONNECTION` can blindly retire the connection a given
request rode, but without age/identity visibility it cannot drive the
age-conditional poisoning `ConnectionInfo` models; see §7.5.) A response from
this transport carries no `ConnectionInfo`. Where fidelity is "advisory" or
"approximate", the gap is a property of WinHTTP's opaque pool, not of this
transport.

### 7.5 Connection lifetime (bounded connection age)

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
`Fixed`/`PerConnection`; it keeps one session for the transport's lifetime.**
(`WINHTTP_OPTION_EXPIRE_CONNECTION` remains a candidate for a different feature:
error-driven poisoning of a connection after a protocol failure.)

Silently ignoring the option would let a caller who configured a bounded
connection age (for cert rotation or load-balancer rebalancing) believe a
guarantee is in force when it is not. But hard-erroring is also wrong: the option
arrives through the generic `fetch` `ConnectionPoolOptions` that callers set
transport-agnostically, and rejecting a config that works on `fetch_hyper` would
break `fetch_winhttp` as a drop-in transport. The compromise: at build time, if
`connection_lifetime` is `Fixed`/`PerConnection`, the transport emits a `warn`-level
`tracing` event (and a telemetry counter) recording that the setting is not
honored, then proceeds. The limitation is thereby visible in logs/telemetry rather
than silent, without failing otherwise-valid clients. A future version may add a
proper mechanism (most likely whole-session recycling gated behind an explicit
opt-in, given its cost and coarse granularity). That this option arrives from the
`fetch` layer at all, rather than being configured on the transport that owns the
connections, is noted as `fetch` API feedback in §17.

## 8. HTTP protocol negotiation

Protocol selection comes from `TransportOptions.supported_http_versions`
(`Vec<http::Version>`), mapped onto WinHTTP request options:

- HTTP/1.1 is WinHTTP's baseline and is always available unless explicitly
  disallowed (below).
- HTTP/2 is enabled by
  `WinHttpSetOption(WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL, WINHTTP_PROTOCOL_FLAG_HTTP2)`.
- HTTP/3 is enabled by the analogous `WINHTTP_PROTOCOL_FLAG_HTTP3`. HTTP/3 is a
  first-class, supported mode, not an opt-in experiment: modern Windows ships it,
  and enabling it is a single protocol flag. QUIC reachability is a runtime
  property (a forced-h3 request against an unreachable QUIC endpoint fails with
  `0x2EFE`/`0x2EFD`), which is a negotiation outcome, not a build gate.

ALPN is performed by Schannel during the TLS handshake; there is no manual ALPN
wiring. The negotiated version is read back after `HEADERS_AVAILABLE` via
`WINHTTP_OPTION_HTTP_PROTOCOL_USED` and set on the `HttpResponse`, so upstream
telemetry reflects what was actually negotiated rather than what was requested.

**Version-set semantics** (`supported_http_versions` -> options), applied by
`options::apply_protocol`:

- Contains `HTTP_11`: baseline allowed.
- Contains `HTTP_2`: set the HTTP/2 flag.
- Contains `HTTP_3`: set the HTTP/3 flag.
- Does not contain `HTTP_11` (only h2 and/or h3): additionally set
  `WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED = TRUE`, which disables the HTTP/1.1
  fallback so only the enabled newer protocols are used. This is how an
  "HTTP/2-or-newer only" (or HTTP/3-only) mode is expressed; if negotiation
  cannot reach a required protocol the request fails rather than downgrading.
- Empty list: use the `fetch` default. `fetch`'s `TransportOptions::default`
  sets `supported_http_versions = [HTTP_11, HTTP_2]`, and an empty list is
  `fetch`'s documented "no explicit preference" signal, so we apply the same
  default (HTTP/1.1 baseline + HTTP/2 enabled, no required-protocol restriction).
- Unmappable entries: WinHTTP speaks only HTTP/1.1, /2, and /3. A version WinHTTP
  cannot express (`HTTP/0.9`, `HTTP/1.0`) is rejected at request construction with
  an `invalid_request` error rather than being silently dropped - silently
  ignoring it could, for a single-element list like `[HTTP_10]`, leave *no*
  protocol selected. A list containing only unmappable versions is likewise an
  error, not a fall-through to the default.

## 9. TLS

TLS is performed by Schannel; `fetch_winhttp` only configures it via
`WinHttpTlsConfig` (§1.2). Knobs are applied with `WinHttpSetOption` on the
request handle before `WinHttpSendRequest`:

- **`https` selection.** `WINHTTP_FLAG_SECURE` on `WinHttpOpenRequest` for
  `https://` targets. `http://` is only issued when the client is built with
  `insecure_allow_http()` and the request filter admits it, identical policy to
  the other transports.
- **Insecure mode.** `accept_invalid_certs` / `accept_invalid_hostnames` set
  `WINHTTP_OPTION_SECURITY_FLAGS` with the relevant
  `SECURITY_FLAG_IGNORE_UNKNOWN_CA | IGNORE_CERT_CN_INVALID |
  IGNORE_CERT_DATE_INVALID | IGNORE_CERT_WRONG_USAGE` bits. This is the insecure
  mode called out in the requirements; it is opt-in and documented as dangerous.
- **Trust source.** The Windows certificate stores and enterprise trust policy.
  No userland root bundle is shipped or consulted.
- **Server certificate inspection / pinning.** Beyond accept/reject, not offered
  in v1. If needed later it hooks the `SECURE_FAILURE` callback and a
  post-handshake `WINHTTP_OPTION_SERVER_CERT_CONTEXT` query.

### 9.1 Client certificates (mTLS) are out of scope for v1

`fetch` does not require a transport to support client certificates: its mTLS
surface (`fetch::tls::ClientIdentity`) travels inside the generic `TlsOptions`
that `fetch_winhttp` deliberately ignores (§1.2), and a transport that offers no
client identity is a conforming `fetch` transport. Client certificates are an
uncommon feature that the large majority of callers never use, and wiring them
into Schannel (importing a DER chain plus PKCS#8 key into an in-memory store,
producing a `PCCERT_CONTEXT`, attaching it with
`WINHTTP_OPTION_CLIENT_CERT_CONTEXT`, and managing hardware-backed identities) is
a self-contained chunk of work with its own lifetime and ownership concerns.

v1 therefore does not implement client certificates. `WinHttpTlsConfig` exposes
no client-identity field. If a concrete need appears, a later iteration can add a
WinHTTP-specific client-identity type and the Schannel import path; it is
entirely possible the WinHTTP transport never needs to support them.

## 10. WinHTTP-managed HTTP behavior

WinHTTP handles several HTTP behaviors internally. This chapter covers how we
configure each so the transport behaves consistently with the rest of `fetch`:
compression (below), redirects (§10.1), and cookies/authentication (§10.2).

- **Automatic decompression (always on).**
  `WinHttpSetOption(WINHTTP_OPTION_DECOMPRESSION, WINHTTP_DECOMPRESSION_FLAG_GZIP
  | WINHTTP_DECOMPRESSION_FLAG_DEFLATE)` makes WinHTTP advertise
  `Accept-Encoding: gzip, deflate`, transparently decode the response, and strip
  `Content-Encoding`/`Content-Length`. The bytes we stream up are already
  decoded.
- **Interaction with `fetch`.** `fetch` has no built-in content decoding
  (`fetch`/`http_extensions`/`seatbelt` carry only content-encoding error labels,
  no gzip/deflate logic), so there is no double-decode risk: WinHTTP is the sole
  decoder, and because it removes `Content-Encoding` the pipeline and callers see
  a plain body. No opt-out is exposed in v1, since exposing one would only hand
  callers an encoded body that nothing downstream is prepared to decode.
- **Brotli/zstd.** Not supported by WinHTTP. We neither advertise nor decode them;
  such responses arrive still-encoded with `Content-Encoding` intact and pass
  through verbatim.
- **Request-body compression.** Not performed automatically; a caller that sets
  `Content-Encoding` and pre-encodes its body has it sent as-is.

### 10.1 Redirects

WinHTTP follows redirects automatically by default, but `fetch` has no built-in
redirect following (its documentation shows following redirects as explicit
application-level logic), and `fetch_hyper` does not follow them. To keep
behavior consistent across transports, `fetch_winhttp` sets
`WINHTTP_OPTION_REDIRECT_POLICY = WINHTTP_OPTION_REDIRECT_POLICY_NEVER`, so
redirect responses (3xx) are surfaced to the caller unchanged, as they are with
`fetch_hyper`. No knob re-enables WinHTTP-managed redirects: exposing one would
diverge from the other transports' behavior for no contract that requires it.

### 10.2 Cookies and automatic authentication are disabled

By default WinHTTP maintains a per-session cookie store and performs automatic
authentication (transparently answering `WWW-Authenticate` challenges with cached
or ambient credentials). Both are session-scoped, and because the session is
shared across cores and requests (§4.2), leaving them on would let cookies and
credentials from one request leak into unrelated requests - a correctness and
security hazard, and a behavior neither `fetch` nor `fetch_hyper` has (hyper is a
stateless transport with no cookie jar). Each request therefore sets:

- `WINHTTP_OPTION_DISABLE_FEATURE` with `WINHTTP_DISABLE_COOKIES` - WinHTTP neither
  stores `Set-Cookie` nor auto-attaches `Cookie`; those headers pass through as
  plain header data for the caller to manage.
- `WINHTTP_OPTION_DISABLE_FEATURE` with `WINHTTP_DISABLE_AUTHENTICATION` - WinHTTP
  does not intercept 401/407 or attach credentials; challenge responses are
  surfaced to the caller unchanged.

The transport is thus stateless between requests, matching the other transports;
any auth or cookie behavior is the caller's responsibility via explicit headers.

## 11. Request/response body streaming

Bodies are modeled as `bytesbuf_io` streams, then bridged to `fetch`'s
`http_body::Body` model. Response (read) buffers are drawn from the client's
`bytesbuf::mem::GlobalPool`. Request (write) buffers are whatever `BytesView` the
caller supplies in the outgoing `HttpBody`; they need not come from that pool.
Either way WinHTTP is agnostic to where the memory came from: it borrows the
pointer for the duration of one async operation (§5) and never allocates or
accounts for it, so the transport imposes no allocation-source requirement on
request bodies.

### 11.1 Outgoing request body -> `bytesbuf_io::Write`

`HttpRequest`'s body is an `HttpBody: http_body::Body<Data = BytesView>`
(pull-based `poll_frame`). The WinHTTP write side is a `bytesbuf_io::Write`:

```rust,ignore
impl bytesbuf_io::Write for WinHttpBodyWriter {
    type Error = HttpError;
    async fn write(&mut self, data: BytesView) -> Result<(), HttpError> {
        // Stores `data` in RequestContext.write_buffer, issues WinHttpWriteData,
        // and awaits WRITE_COMPLETE via an events_once step. The BytesView stays
        // pinned until the callback fires (§5).
    }
}
```

The sending strategy is chosen in `RequestDriver::send_body`. In **both** cases
`WinHttpSendRequest` is called with a `NULL`/zero `lpOptional` buffer; the request
body is never passed inline to `WinHttpSendRequest` and is always streamed with
`WinHttpWriteData`. This sidesteps the `lpOptional` lifetime rule (an inline
optional buffer would have to stay valid until `SENDREQUEST_COMPLETE`) and keeps a
single body-writing path:

- **Known length** (buffered body / `Content-Length`): the total length passed to
  `WinHttpSendRequest` (`dwTotalLength`) is a `DWORD`. When the known length fits
  in `u32`, pass it directly and stream frames through `WinHttpBodyWriter`. When it
  exceeds `u32::MAX`, `dwTotalLength` cannot represent it, so instead pass
  `WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH` and set an explicit 64-bit `Content-Length`
  request header (WinHTTP honors a caller-supplied `Content-Length` and does not
  fall back to chunked when it is present), then stream the body identically. Either
  way there is one write path.
- **Unknown length** (streaming body): `WinHttpSendRequest` with
  `WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH` and a `NULL` optional buffer; WinHTTP emits
  chunked encoding on HTTP/1.1. Each `poll_frame` chunk is pulled and written;
  end-of-body is signaled by proceeding to `WinHttpReceiveResponse`.

Each individual `WinHttpWriteData` length is also a `DWORD`, so a single
`BytesView` larger than `u32::MAX` is written in `u32`-sized slices across
successive `WRITE_COMPLETE` steps, independently of how the total length is
declared above.

### 11.2 Incoming response body <- `bytesbuf_io::Read`, then an `http_body::Body`

The read side is a `bytesbuf_io::Read` over WinHTTP:

```rust,ignore
impl bytesbuf_io::Read for WinHttpBodyReader {
    type Error = HttpError;
    async fn read_more_into(&mut self, into: BytesBuf) -> Result<(usize, BytesBuf), HttpError> {
        // 1. WinHttpQueryDataAvailable -> DATA_AVAILABLE(n)
        // 2. move `into` to RequestContext.read_buffer, hand WinHTTP its dst ptr,
        //    reading min(n, remaining capacity) bytes
        // 3. WinHttpReadData -> READ_COMPLETE { buffer, len }
        // 4. return (len, buffer)   // ownership of the BytesBuf comes back here;
        //    len == 0 => EOF
    }
}
```

EOF is taken from a **zero-length `READ_COMPLETE`**, not from
`WinHttpQueryDataAvailable` returning 0. Both usually coincide, but WinHTTP's
documented completion signal is the zero-length read, and reading directly avoids
depending on `QueryDataAvailable`'s value for correctness. `QueryDataAvailable` is
still used to right-size the read (so we never issue an oversized `ReadData`), but
the authoritative "body finished" decision is a `READ_COMPLETE` with `len == 0`.

The natural bridge to `fetch` is `ReadExt::into_futures_stream`, which turns a
`bytesbuf_io::Read` into a `Stream<Item = Result<BytesView>>` that
`HttpBodyBuilder::stream` accepts directly. This requires `ReadAsFuturesStream<S>`'s
boxed in-flight read future to be `Send` (so the stream satisfies
`HttpBodyBuilder::stream`'s `Send + 'static` bound), which `bytesbuf_io` provides.
It is sound because `bytesbuf_io::Read` is `#[trait_variant::make(Send)]` and its
read futures are already `Send`. The response body is then simply:

```rust,ignore
let stream = WinHttpBodyReader::new(/* .. */).into_futures_stream();
let body = builder.stream(stream); // HttpBodyBuilder::stream, Send-clean
```

No hand-written `http_body::Body` is needed. The resulting `HttpBody` is pull-based:
WinHTTP reads are issued lazily as the consumer polls, so backpressure is natural and
there is no unbounded buffering. A
body-timeout from request extensions is honored via the same `HttpBodyOptions`
path `fetch_hyper` uses (`HttpBodyBuilder::stream(.., options)` wraps the body in
an idle timeout).

The `READ_COMPLETE` buffer WinHTTP fills is a slice reserved inside a pooled
`BytesBuf`; it stays pinned until the callback fires (§5), then the filled prefix
is yielded as a zero-copy `BytesView`.

### 11.3 End-to-end request lifecycle (`RequestDriver` in `request.rs`)

`WinHttpTransport::execute(req)` returns a `Send` future implemented by
`RequestDriver`, which runs (each `->async` awaits an `events_once` completion;
§4.3):

```text
translate req (method/uri/headers -> UTF-16)
  -> open connect handle (inline WinHttpConnect; non-blocking, no cache, §7.1)
  -> WinHttpOpenRequest + set options (protocol, decompression, redirect, cookies/auth off, security, timeouts)
  -> set RequestContext pointer as WINHTTP_OPTION_CONTEXT_VALUE
  -> WinHttpSendRequest ->async SENDREQUEST_COMPLETE
  -> [streaming body] loop poll_frame -> WinHttpBodyWriter.write ->async WRITE_COMPLETE
  -> WinHttpReceiveResponse ->async HEADERS_AVAILABLE
  -> WinHttpQueryHeaders (status, negotiated version, header block)  [sync]
  -> build HttpResponse { parts, HttpBody streamed from WinHttpBodyReader }
  -> return Ok(response)   // body streamed lazily by the caller
```

Header translation is mechanical: request headers serialize to a WinHTTP CRLF
header blob; the response `WINHTTP_QUERY_RAW_HEADERS_CRLF` blob parses back into
an `http::HeaderMap`. Method and URI come from the `http::Request` parts.

**Response-body handle ownership.** The request handle must outlive `execute`'s
return, because the body is read lazily *after* the driver returns the
`HttpResponse`. So at the point the response is built the `RequestGuard` **moves
into** the `WinHttpBodyReader`, carrying the request handle, its per-request connect
handle, the shared session `Arc<WinHttpSession>`, and the `RequestContext` pointer
with it. Holding the session `Arc` in every live request (driver or body reader)
keeps the session handle alive for exactly as long as any request needs it.

Whoever owns the guard when the request ends runs the single synchronous teardown of
§5.3: the body reader on EOF (a zero-length `READ_COMPLETE`, §11.2), on error, or on
drop; or the driver itself when there is no response body (HEAD, 204) and it never
hands off a guard. This is the one close authority on every path - exactly one owner,
closing exactly once.

## 12. Timeouts and time

`fetch` enforces most timeouts above the transport; WinHTTP provides native
timers for the transport-owned steps. The transport owns exactly one timeout that
`fetch` does not enforce for us: connect.

### 12.1 Mapping `fetch` timeouts to WinHTTP

| `fetch` concept | Type / default | Where enforced | WinHTTP equivalent |
|-----------------|----------------|----------------|--------------------|
| Connect timeout | `TransportOptions.connect_timeout` (30 s) | This transport (`fetch` core does not wrap connect; `fetch_hyper` enforces it in its own connector) | `WINHTTP_OPTION_CONNECT_TIMEOUT`, applied to the send-time TCP/TLS handshake |
| Response timeout | `http_extensions::ResponseTimeout` | Above transport, in `fetch::HttpClient::execute` (wraps the whole pipeline, maps to `HttpError::timeout`) | `WINHTTP_OPTION_RECEIVE_RESPONSE_TIMEOUT` as backstop |
| Body idle timeout | `http_extensions::BodyTimeout` | We apply it, like `fetch_hyper`, via `HttpBodyOptions::timeout` on the response body (§11.2) | `WINHTTP_OPTION_RECEIVE_TIMEOUT` per read as backstop |
| Seatbelt request timeout | `seatbelt::TimeoutLayer` (30 s) | Above transport | n/a |
| Resolve/send timeouts | (no distinct `fetch` concept; transport-specific by design, §17) | this transport | `WinHttpSetTimeouts` resolve/send fields, set from `WinHttpOptions` |

`WinHttpSetTimeouts(resolve, connect, send, receive)` sets the four base timers;
`WINHTTP_OPTION_RECEIVE_RESPONSE_TIMEOUT` is set separately and is forced by
WinHTTP to be at least the receive timeout. Each of these is a native WinHTTP timer,
scheduled by WinHTTP inside its own async machinery (§3.1); the transport's own
connect deadline (§12.2) is the sole exception.

"Connect timeout" is a universal concept `fetch` already models
(`TransportOptions.connect_timeout`) but leaves each transport to enforce, which
is why this transport wires it to `WINHTTP_OPTION_CONNECT_TIMEOUT`. Resolve and
send timeouts have no distinct `fetch` concept, so they are exposed as
transport-specific `WinHttpOptions` knobs - the appropriate home for
fine-grained network-phase timers, as discussed in the `fetch` API feedback (§17).

### 12.2 One transport-scheduled delay: the outer connect deadline

`WINHTTP_OPTION_CONNECT_TIMEOUT` bounds a single TCP connection *attempt*. For a
multi-homed host (several A/AAAA records) WinHTTP tries addresses in turn, and for
transient failures it may retry, so the *total* wall-clock time to establish a
connection can exceed `TransportOptions.connect_timeout` even though every
individual attempt honored the native per-attempt timer. To make
`connect_timeout` behave as the total deadline `fetch` callers expect, the driver
races the connect/send phase against a single `tick::Clock::delay(connect_timeout)`
using the clock already threaded in from `CustomContext` (the same clock the body
builder uses; no new dependency). The raced phase runs up to
`SENDREQUEST_COMPLETE`, which covers name resolution, the TCP/TLS connect, proxy
discovery, and submission of the request line and headers. Because this transport
always streams the request body with `WinHttpWriteData` *after*
`SENDREQUEST_COMPLETE` (§11.1), the body transfer is outside this deadline and is
governed by the send/body timers instead. Whichever resolves first wins: on
the delay firing, the driver closes the request handle (which cancels the in-flight
connect/send, §5.3) and returns `HttpError::timeout`; on the connect/send
completion, the delay future is dropped. One consequence worth stating: the deadline
can fire after the request headers reached the server, so for a bodyless
non-idempotent request the peer may already have begun processing. Deciding whether
such a request is safe to retry is `seatbelt`'s concern (idempotency/retry policy),
not the transport's; the transport only reports the timeout.

This is the transport's *only* self-scheduled delay. The native per-attempt
timers (`WINHTTP_OPTION_CONNECT_TIMEOUT` inner, plus the resolve/send/receive
timers) remain in force; the `tick::Clock` race sits *outside* them as the total
budget. Every other timeout is either a native WinHTTP timer for a step WinHTTP
owns, enforced above the transport by `fetch`/`seatbelt`, or applied to the
response body via `HttpBodyOptions` (§11.2). `tick::Clock` is the sole source of
time on this path; the transport never calls `std::thread::sleep` or `tokio::time`.

### 12.3 What is controllable in tests

Timeout *configuration* is asserted in unit tests: the mock bindings record the
`WinHttpSetTimeouts` arguments and the `WINHTTP_OPTION_CONNECT_TIMEOUT` /
`WINHTTP_OPTION_RECEIVE_RESPONSE_TIMEOUT` set-option calls, so a test asserts that
each `fetch` timeout option is translated into the correct WinHTTP timer value.

The one transport-scheduled delay - the outer connect deadline (§12.2) - *is*
driven by `tick::Clock`, so it is unit-testable deterministically: with a mock
clock and mock bindings that never complete the connect, a test advances the clock
past `connect_timeout` and asserts the driver closes the handle and yields
`HttpError::timeout`, and conversely that a connect completing before the deadline
drops the delay without firing.

Timeout *firing against the real OS* cannot be made deterministic: the real
WinHTTP path uses the real OS clock, which the tests cannot freeze or
fast-forward. Real-time integration tests are therefore unacceptable (they would
be flaky). Integration tests instead configure timeouts large enough that they can
never fire during a healthy run, so a timeout tripping is always a genuine failure
signal, never a timing race. Verifying that a given deadline *does* fire is left
to the unit path (mock clock for the connect deadline; assert the option is set
for the native timers) rather than to any wall-clock integration test.

## 13. Error handling model

`fetch` transports return `Result<HttpResponse, HttpError>`. `HttpError`
(`http_extensions`) carries a source error, an `ohno::ErrorLabel`, and a
`recoverable::RecoveryInfo`, mirroring `fetch_hyper`:

- **Error surface.** Two Win32 error sources: the last-error from a failing
  synchronous call, and `dwError` from `WINHTTP_ASYNC_RESULT` on a
  `REQUEST_ERROR` callback. `SECURE_FAILURE` supplies a bitmask of certificate
  problems that we capture and attach.
- **Mapping.** A single `error.rs` function turns a Win32/`WINHTTP_*` code into
  `HttpError::other(WinHttpError { code, .. }, recovery, label)`.
- **Labels** (`error_labels.rs`, mirroring `crates/fetch/src/error_labels.rs`):

  | Condition | `ErrorLabel` |
  |-----------|--------------|
  | `ERROR_WINHTTP_CANNOT_CONNECT`, `NAME_NOT_RESOLVED` | `connect` |
  | `ERROR_WINHTTP_TIMEOUT` | `timeout` |
  | `ERROR_WINHTTP_SECURE_FAILURE` and secure-failure bits | `tls` |
  | `ERROR_WINHTTP_OPERATION_CANCELLED` | `abandoned` |
  | send/receive/protocol failures | `request_winhttp` |

### 13.1 Recoverability rationale

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

## 14. Test plan

The bindings facade (§2) makes the transport testable at two levels. Unit tests
drive the `RequestDriver` against `MockBindings` with synthesized callbacks;
integration tests exercise the real OS against a localhost server. No test depends
on Tokio directly.

**Miri.** The FFI path cannot run under Miri, so the cancellation/leak invariants
(§5) are asserted on the mock path, which can, plus the real integration tests.
The mock path exercises the same allocate/leak/reclaim code as production so that
"free exactly once on `HANDLE_CLOSING`" is verified where Miri is available;
integration tests are `#[cfg_attr(miri, ignore)]`. Miri coverage is not a separate
concern from the test strategy but a property of it: the mock-bindings unit tests
are written so the ownership-critical paths run and are checked under Miri.

### 14.1 Unit tests (mock bindings, no network)

A `TestHarness` builds the transport over `MockBindings` and exposes
`complete(op, result)` to fire the callback the real trampoline would fire. This
deterministically interleaves completions and injects cancellations and
out-of-order callbacks.

### 14.2 How each key factor is tested

Most factors are checked by asserting the calls and values the driver hands the mock.
The three with nontrivial ordering - reentrant completion, cancellation / FFI
ownership, and body streaming - need scripted sequences and are described as prose
after the table.

| Factor | Key assertions | Notable adverse / edge case |
|--------|----------------|-----------------------------|
| Threading (§4) | completions fired from a foreign OS thread reach the awaiting future; `static_assertions` for `execute`'s future `Send`, handles `Send`+`!Sync`, handler `Send + Sync`, and per-core-owned pools | all setup calls run inline on the caller's thread |
| Error handling (§13) | table-driven Win32/`WINHTTP_*` code -> `ErrorLabel` + `RecoveryInfo`; `GetLastError` mapping on a failing synchronous call | a 4xx/5xx response is `Ok`, not `Err` |
| Protocol negotiation (§8) | protocol-flag bitmask + `HTTP_PROTOCOL_REQUIRED` per `supported_http_versions` (empty -> `fetch` default; h2/h3-only -> required); response `Version` from the queried negotiated protocol | unmappable version (`HTTP/1.0`, `HTTP/0.9`) rejected as `invalid_request` |
| TLS (§9) | `WINHTTP_FLAG_SECURE` iff `https`; security-flags bitmask per `accept_invalid_*`; `SECURE_FAILURE` -> `tls`-labeled, non-retryable | mTLS out of scope (§9.1) - nothing to assert |
| Compression / redirects / statelessness (§10) | `DECOMPRESSION`, `REDIRECT_POLICY_NEVER`, `DISABLE_COOKIES`, `DISABLE_AUTHENTICATION` set; an already-decoded body streams untouched; a 3xx is surfaced verbatim | brotli/zstd response passes through still-encoded |
| Connection management (§7) | connect handle opened per request and closed with it; max-conns mapping; keep-alive left enabled; `DISABLE_GLOBAL_POOLING` on the session | `connection_lifetime` Fixed/PerConnection: accepted, no recycling, emits the `warn` "not honored" event |
| Timeouts (§12) | `WinHttpSetTimeouts` + connect/response options get values derived from `fetch` options; mock-clock connect deadline (§12.2): advance past `connect_timeout` -> handle closed + `HttpError::timeout` | a connect completing first drops the delay unfired |

- **Inline / reentrant completion.** Configure `MockBindings` so an async call
  (e.g. `read_data`) fires its completion *synchronously, inline, on the submitting
  thread* before returning - the reentrant case `ASSURED_NON_BLOCKING_CALLBACKS`
  permits (§3.1). Assert the driver still observes the result correctly (the
  `events_once` send lands before the receiver is awaited) and that no borrow of
  `RequestContext` is held across the submit (§5.5). Also assert the
  `SECURE_FAILURE`-then-`REQUEST_ERROR` sequence records the cert flags without
  consuming the sender early.
- **Cancellation and FFI ownership.** The centerpiece. (1) Drop the response
  body while a `READ_COMPLETE` is outstanding; assert `close_handle` is called and the pooled
  `RequestContext` is not returned to the pool until the harness fires the
  synthetic `HANDLE_CLOSING`, then that it is returned exactly once (the mock
  records alloc/free; run under Miri where available). (2) Cancel with an outstanding
  write. (3) `ERROR_WINHTTP_OPERATION_CANCELLED` delivered after close is swallowed
  (no waiter) without UB. (4) The pooled `Box` and rented `events_once` events return
  to their per-core pools. (5) **Setup-failure leak-freedom:** fail the context
  set-option (`WINHTTP_OPTION_CONTEXT_VALUE`); the `Box` returns to the pool inline,
  no leak. (6) **Null-context guard:** a `HANDLE_CLOSING` for a handle whose context
  was never installed (early-failed request, or a connect/session handle) is ignored
  and reconstructs no `Box`. (7) The session-level status callback is registered once
  with the full notification mask (§5.3). (8) **Session lifetime:** drop the last
  transport instance / session `Arc` while a response body is mid-read; assert the
  body reader's retained session `Arc` keeps the session wrapper alive so the in-flight
  read completes, and that at guard drop the `Arc` is released synchronously while
  WinHTTP's native parent refcount carries the OS session through the final
  `HANDLE_CLOSING` (run under Miri where available).
- **Body streaming.** Drive the `bytesbuf_io::Read` adapter with a scripted
  `DATA_AVAILABLE`/`READ_COMPLETE` sequence and assert EOF is taken from a
  zero-length `READ_COMPLETE` (not from `QueryDataAvailable`), that `ReadComplete`
  returns the same pooled `BytesBuf` (ownership round-trip) with the correct appended
  length, and that no read is issued until the consumer polls `poll_frame`
  (backpressure); also a mid-stream error. For the writer, script `WRITE_COMPLETE`s
  across frames and assert chunk-by-chunk `WinHttpWriteData` with correct
  pointers/lengths and buffer pinning, a `BytesView` larger than `u32::MAX` split
  into `u32`-sized writes, and a known body length above `u32::MAX` using
  `WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH` plus an explicit `Content-Length` (§11.1).

### 14.3 Integration tests (real WinHTTP, localhost)

Gated behind `#[cfg(windows)]` and `#[cfg_attr(miri, ignore)]`, against a
localhost server (a small `std::net`-based server, or `wiremock` as used
elsewhere in `fetch`). These validate the real OS path end to end:

- GET/POST with small and large bodies; response body correctness and size.
- Streaming upload (unknown length -> chunked) and streaming download; assert
  incremental delivery, not just final bytes.
- Real gzip/deflate responses are transparently decoded.
- `https` against a localhost TLS server with a self-signed cert: fails by
  default, succeeds with `accept_invalid_certs`; a valid-cert case proves normal
  trust works. (Client-certificate/mTLS is out of scope for v1, §9.1.)
- HTTP/1.1 vs HTTP/2 negotiation against a server that supports both (wiremock and
  hyper-based localhost servers cover h1 and h2); assert the reported response
  `Version`.
- HTTP/3: wiremock and hyper localhost servers do not speak HTTP/3, so h3 is
  tested against a localhost QUIC server stood up with the `quinn` + `h3` crates
  (dev-dependencies) using a self-signed cert and `accept_invalid_certs`. Assert
  the negotiated `Version` is HTTP/3, and separately assert the "h3 required but
  QUIC unreachable" path yields the expected failure (`0x2EFE`/`0x2EFD`).
- Connection reuse: two sequential requests to the same authority reuse the
  connection (observable via server-side connection counting).
- Timeout configuration is validated only structurally (unit, §12.3). Integration
  tests set every timeout large enough that it can never fire during a healthy
  run, so a tripped timeout is always a real failure, never a timing race. No
  integration test asserts a timeout *firing* against a slow/black-hole endpoint,
  because that would depend on real wall-clock timing and be flaky.
- Real cancellation: drop an in-flight download future and assert clean teardown
  (no panic, no leak), the integration counterpart to the unit cancellation tests.

The full `fetch` pipeline (retry/breaker/telemetry) is validated by building an
`HttpClient` via `fetch_winhttp::builder(...)` and asserting a real request round-trips,
mirroring `fetch`'s existing `requests` integration test structure.

## 15. Dependencies

Planned crate dependencies (all `default-features = false`, per workspace policy):

- `windows` `0.62.2` (added to `[workspace.dependencies]`; oxidizer currently
  vendors only `windows-sys`), features `Win32_Networking_WinHttp` and
  `Win32_Foundation`, target-gated via `[target.'cfg(windows)'.dependencies]`.
- `fetch`, `http_extensions`, `fetch_options`, `bytesbuf`, `bytesbuf_io` (whose
  `ReadExt::into_futures_stream` feeds the response body, §11.2), `thread_aware`,
  `tick`, `events_once`, `plurality`,
  `ohno`, `recoverable`, `http`, `http-body`, `opentelemetry` (meter), `tracing`,
  `widestring` (UTF-16), `smallvec`. No `anyspawn`: nothing the transport calls
  can block (§3.1), so there is no blocking pool and no `Spawner`.
- `events_once` provides the reusable one-shot event pool
  ([folo-rs/folo](https://github.com/folo-rs/folo)); its `EventPool<T>` is
  `Send + Sync`, cheaply clonable, and returns rented endpoints to the pool on
  drop with no external access (§6).
- `plurality` provides the FFI raw-pointer round-trip (`Box::into_raw`/`from_raw`,
  §5.3).
- Dev: `mockall`, `static_assertions`, a localhost test server (`wiremock` or a
  hand-rolled `std::net` server), `quinn` + `h3` (localhost HTTP/3 server),
  `testing_aids`.

No Tokio dependency in non-dev code, and no `anyspawn` at all: the transport runs
its inline setup calls directly and relies on WinHTTP's own worker threads for
async completions.

## 16. API-contract vs implementation detail

Per repo convention, only the following are contractual and belong in API docs:
`fetch_winhttp::builder`/`fetch_winhttp::new`, `WinHttpDeps`, `WinHttpTlsConfig`,
`WinHttpOptions`, and the guarantee that the produced client is a standard
`fetch` `HttpClient`. The observable HTTP behaviors this transport must exhibit -
protocol negotiation (§8), transparent decompression (§10), redirect and
cookie/auth handling (§10.1-§10.2), timeout semantics (§12), and error
label/recoverability mapping (§13) - are also part of the behavioral contract, to
the extent `fetch` specifies them for any conforming transport. What is *not*
contractual is the machinery that achieves those behaviors: everything in §2
through §7 (the bindings facade, the pooling scheme, the `RequestContext` lifetime
protocol, the callback trampoline, handle wrappers, connection-management
internals) plus the specific WinHTTP options and callbacks behind the behaviors
above. These are implementation detail and are not promised in public
documentation.

## 17. Feedback on the `fetch` API

Building this transport surfaced a few places where the current `fetch` API is
shaped around its original Hyper/Tokio transport and makes assumptions that do not
hold for every transport. None blocks this design, but each forces a workaround
worth recording for a future `fetch` revision.

The common thread is a layering question: `fetch` is fundamentally a *pipeline
assembler* (it builds the HTTP request/response pipeline and its middleware), while
the *transport* is the component that actually performs network communication.
Configuration should be split along that line - anything that governs how bytes
travel over the network (TLS, the network-phase timeouts, connection lifetime and
reuse) is a transport concern and belongs on the transport, while end-to-end pipeline
concerns (overall response deadline, retry, telemetry) stay in `fetch`. Several of
the items below are instances of that split being in the wrong place today.

- **No first-class way to plug in a downstream transport.** `HttpClient` and its
  builder live in `fetch`, so a transport crate cannot add an inherent
  `HttpClient::winhttp(..)` constructor (orphan rule). We expose free functions
  (`fetch_winhttp::builder`/`new`) instead, which is fine but asymmetric: transports
  that happen to live in `fetch` get first-class methods, downstream ones get
  bolted-on free functions. A transport-agnostic API where the caller always
  explicitly constructs and plugs in a transport - e.g.
  `HttpClient::build(fetch_winhttp::transport().tls(cfg))` - would be more
  predictable, avoid a Hyper-colored default, and treat every transport uniformly.
  The slightly more verbose hello-world is worth the consistency.

- **TLS configuration is over-abstracted.** `fetch`'s generic `TlsOptions`
  carries rustls/native-tls material that only the Hyper transport understands;
  Schannel-based WinHTTP cannot consume it and needs its own knobs (§1.2, §9).
  Different transports inherently support different TLS configuration models - it is
  a fact of life, not a `fetch` design choice, that they cannot be configured
  uniformly. The shortcoming is that `fetch` tries to abstract TLS at the
  transport-agnostic level anyway; it should instead be configured per transport, on
  the transport being plugged in. The same `HttpClient::build(transport().tls(cfg))`
  shape would express this cleanly.

- **Connection-management options live at the wrong layer.** `fetch` exposes
  `max_connections`, `connection_idle_timeout`, and `connection_lifetime` (§7.4), but
  the transport is the component that owns and manages connections - so how (and
  whether) each option can be honored is entirely transport-dependent. WinHTTP owns
  its own pool and exposes no per-connection age control, so `connection_lifetime`
  simply cannot be implemented here (§7.5), while the same options map cleanly onto a
  transport that manages its own sockets. Connection-management configuration
  therefore belongs on the transport that manages the connections, not on `fetch`.

- **Timeouts are over-abstracted, not under-modeled.** `fetch` models a connect
  timeout but has no concept for resolve or send timeouts. That absence is *not* the
  problem: different transports support different sets of fine-grained timers, measure
  them against different phase boundaries (what each timer includes or excludes), or
  cannot express some of them at all, so there is no uniform fine-grained timeout model
  to standardize. The right split is by scope. *End-to-end* pipeline deadlines (the
  overall response timeout, `seatbelt`'s request timeout) belong in `fetch`, above the
  transport. *Network-phase* timers (resolve, connect, send, receive) are inherently
  transport-specific and should be configured on each transport, which is exactly why
  this transport exposes them as `WinHttpOptions` knobs (§12.1) rather than expecting
  `fetch` to model them. The mismatch today is that `fetch` reaches down to model a
  connect timeout while leaving the rest to transports - it should leave all
  network-phase timers to the transport and keep only pipeline-level deadlines.


[`RequestHandler`]: https://github.com/microsoft/oxidizer/tree/main/crates/http_extensions
[WinHTTP]: https://learn.microsoft.com/en-us/windows/win32/winhttp/using-winhttp
