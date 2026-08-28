# `fetch_winhttp` implementation

Status: design, pre-implementation. This document describes the implementation strategy of the `fetch_winhttp` crate: the OS bindings facade, the WinHTTP asynchronous model, the threading and cancellation/FFI-ownership machinery, object pooling, body-streaming mechanics, and the test plan. The higher-level architecture, behavior, and design tenets are documented separately in [design.md](design.md). The crate currently ships only these design documents and a placeholder `lib.rs`.

## Architecture at a glance

This is the whole design in one picture; the numbered chapters below elaborate each part.

- **One OS session per (core × pool slot), scoped to the built client.** Each per-core
  transport instance opens its own session - one per configured `multiple_pools` slot,
  a single slot by default - when `fetch` materializes it, so two independently
  built clients (or clones) never share a session or its connection pool (§3.2).
- **One transport instance per core.** `fetch` clones and relocates the transport
  per core (`Isolation::Isolated`, §3.2); each instance owns its object and event
  pools (§5) and one session `Arc` per pool slot.
- **One shared request state with directional drivers** (§4.4). Setup owns the
  `RequestGuard` and rents a pooled `RequestContext`; after headers are sent, independent
  upload and response drivers share the guard and use separate callback slots (§4.1).
- **WinHTTP drives the I/O on its own threads** (§3). The transport issues
  asynchronous calls and each one signals completion back to the awaiting future
  through an `events_once` one-shot (§3.3). A completion runs either inline on the
  submitting thread (keeping work on one processor) or on a WinHTTP worker thread.
- **No blocking pool and no Tokio.** Every setup call is synchronous but performs no
  I/O, so it runs inline on the executor; only WinHTTP's own async steps defer
  (§2.1).
- **Ownership across FFI is callback-driven** (§4). Dropping the guard closes the
  handles synchronously, but the `RequestContext` is freed only on WinHTTP's final
  `HANDLE_CLOSING` callback, which guarantees no use-after-free under cancellation.

## 1. The bindings facade (OS abstraction for testability)

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
    fn query_trailers(&self, h: RawHandle) -> Result<http::HeaderMap>; // WINHTTP_QUERY_FLAG_TRAILERS after EOF
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
  RequestContext`, §4.2), and all recording/expectation state lives in the
  `Arc<MockBindings>` the harness owns via the `Facade`. Nothing is a global
  singleton. See §7.

**Safety contract of the `Bindings` API.** Because callers drive raw OS handles and
async buffer lifetimes through this trait, a small set of caller-side invariants
must hold for every impl (production or mock) to be sound. They are stated once here
and relied on throughout §4 and §6:

- A buffer handed to `write_data`/`read_data` must stay valid and untouched until
  that operation's completion callback fires (WinHTTP borrows it asynchronously).
- The `RequestContext` must be fully populated and every borrow of it dropped
  **before** the async call is issued, so the completion (possibly reentrant, §2.1)
  has exclusive access via the context pointer.
- At most one send operation and one receive operation are outstanding per request handle
  at a time. The two directional lanes may overlap after request headers are sent.
- The status callback must be registered (with the handle-close flag) and the
  context installed before the first async call, and each handle is closed exactly
  once (§4.3).

### 1.1 Crate/module layout

```text
crates/fetch_winhttp/
  src/
    lib.rs               // #![cfg(windows)] gate + re-exports + crate docs
    builder.rs           // WinHttpDeps, WinHttpOptions, builder()/new()
    transport.rs         // WinHttpTransport: per-core Service<HttpRequest> (§3.2)
    session.rs           // per-core session handle (§3.2)
    request.rs           // RequestDriver: one request/response lifecycle
    context.rs           // RequestContext (per-operation FFI context; pooled)
    callback.rs          // extern "system" trampoline -> dispatch_completion
    body/
      read.rs            // bytesbuf_io::Read over WinHttpReadData (response)
      write.rs           // bytesbuf_io::Write over WinHttpWriteData (request)
    tls.rs               // WinHttpTlsConfig -> security flags
    options.rs           // validated native protocol option mapping
    handle.rs            // RAII handle wrappers (Send/Sync assertions)
    error.rs             // Win32 -> HttpError mapping
    error_labels.rs      // ErrorLabel constants
    bindings/
      abstractions.rs    // Bindings trait (OS entry-point contract)
      facade.rs          // Facade enum (Real / Mock dispatch)
      real.rs            // windows-crate impl (cfg(windows))
      mod.rs             // module wiring only (no type definitions)
  docs/design.md
  docs/implementation.md
```

Because the crate root is gated by `#![cfg(windows)]`, it compiles to an empty
module on non-Windows targets. On non-Windows CI legs the crate therefore builds to
nothing, pulling in no `windows` dependency, so `members = ["crates/*"]` stays green.

## 2. WinHTTP asynchronous model primer

A single request drives this WinHTTP handle chain and callback sequence:

| Step | Call | Sync/async | Completion callback |
|------|------|-----------|---------------------|
| S1 | `WinHttpOpen(WINHTTP_FLAG_ASYNC)` | sync | - (build-time) |
| S2 | `WinHttpSetStatusCallback` (session-level; inherited by all requests) | sync | - (build-time; mask in §4.3) |
| 3 | `WinHttpConnect` | sync, inline (see §2.1) | - |
| 4 | `WinHttpOpenRequest` | sync | - |
| 5 | `WinHttpSetOption`xN (incl. context), `WinHttpSetTimeouts` | sync | - |
| 6 | `WinHttpSendRequest` | async | `SENDREQUEST_COMPLETE` |
| 6a| `WinHttpWriteData` (streaming body, per chunk) | async send lane | `WRITE_COMPLETE` |
| 7 | `WinHttpReceiveResponse` | async receive lane; may overlap 6a | `HEADERS_AVAILABLE` |
| 8 | `WinHttpQueryHeaders` | sync (buffered) | - |
| 9 | `WinHttpQueryDataAvailable` | async receive lane | `DATA_AVAILABLE` (n bytes) |
| 10| `WinHttpReadData` | async receive lane; may overlap 6a | `READ_COMPLETE` (n bytes) then loop 9/10 until 0 |
| 11| `WinHttpCloseHandle` | sync | `HANDLE_CLOSING` (final callback) |

Errors on any async step arrive as `REQUEST_ERROR` carrying a
`WINHTTP_ASYNC_RESULT { dwResult, dwError }`. TLS validation problems also raise
`SECURE_FAILURE` before the `REQUEST_ERROR`.

### 2.1 Synchronous setup calls run inline (no blocking pool)

"Synchronous" in WinHTTP means "returns its result directly rather than via a
completion callback" - not "blocks on I/O". The setup calls the transport makes
(`WinHttpConnect`, `WinHttpOpenRequest`, the `WinHttpSetOption`/`WinHttpSetTimeouts`
family, `WinHttpQueryHeaders`) do no network, DNS, or socket work; they allocate and
configure handles, deferring all I/O to the async steps. They are therefore safe to
run inline on an executor thread, and the transport needs **no** `anyspawn::Spawner`
and no blocking pool.

The sole exception is the very first `WinHttpOpen` in a process, which runs
WinHTTP's one-time global initialization (a lock plus registry reads) and can
briefly block. Since each per-core transport instance opens its own session lazily
inside the factory when `fetch` materializes it (§3.2), this is a one-time
per-core construction cost off the request path; the process-wide global
initialization runs only on the first such open.

The session sets `WINHTTP_OPTION_ASSURED_NON_BLOCKING_CALLBACKS`: we promise our
completion callbacks never block, and in return WinHTTP may invoke a callback
**inline** on the submitting thread whenever an operation completes immediately
(e.g. a read served from an internal buffer), instead of hopping to a thread-pool
worker. We want this - it removes a thread-pool hop on the hot path (§3.1).

The callback trampoline (§4) is safe to run reentrantly because it does a small,
bounded, non-blocking amount of work: recover the `*mut RequestContext`, identify
the directional lane, take that lane's `events_once` sender and buffer, and send
the `CompletionResult`. It
performs no I/O and never waits on WinHTTP. Returning pooled memory (an
`events_once` endpoint, the context `Box`, a `BytesBuf`) on a cancellation or
`HANDLE_CLOSING` path is likewise non-blocking. The one heavier case - the last
context `Box` drop freeing the `plurality` pool's chunks - happens only at shutdown,
where cost is irrelevant and only correctness matters; it is a heap free, not a wait
on WinHTTP, so the assurance still holds.

Reentrancy is sound because of one submitting-side rule (§4.5): the driver fully
populates the `RequestContext` and drops every borrow to it **before** issuing the
async call. However the completion then arrives, it has exclusive access through the
leaked pointer, and the `events_once` send is the single release/acquire edge that
hands buffer ownership back.

## 3. Threading model

Two kinds of threads are in play, and the design is about moving data safely
between them:

1. **Async executor threads** (the caller's runtime). `WinHttpTransport::execute`
   runs here and the returned future is polled here. Every setup call (§2.1) runs
   inline on these threads; `fetch` requires the returned future to be `Send`.
2. **WinHTTP's own worker threads.** When a completion cannot be produced
   immediately, WinHTTP delivers it later on a thread it owns. The application
   neither creates, sizes, nor owns these threads.

A completion is delivered inline on the submitting executor thread or later on a
WinHTTP worker, per §2.1; the callback trampoline (§4) is identical either way, and
there is no third "blocking pool" tier - no request-path call can block.

### 3.1 How WinHTTP schedules callback work

WinHTTP does not run a bespoke thread pool; it posts async completions to the
process-global Win32 thread pool, from which a worker dispatches our callback. There
is no per-request or per-handle thread affinity: successive completions for one
request can land on different workers, and we do **not** assume WinHTTP serializes
callbacks per handle. Soundness rests only on "exactly one completion per async
operation" plus "one operation outstanding per directional lane" (§4.5), with the
status-vs-completion race closed by an atomic (§4.5).

Two consequences shape the design: all per-request callback state must be reachable
from the context pointer alone and safe to touch from any thread (§4.1, with `Send`
handle wrappers, §3.4); and the callback-to-future handoff must be a real
cross-thread signal, not a shared cell - that is `events_once` (§3.3), which behaves
identically same-thread or cross-thread.

Inline completions (§2.1) give a degree of processor affinity for free: when an
operation completes immediately, WinHTTP runs the callback on the very thread that
submitted it, so completion work stays on the same processor as the async work that
issued it rather than hopping to an arbitrary thread-pool worker. Only genuinely
deferred completions incur the hop.

### 3.2 Per-core transport instances and per-core sessions

`fetch_winhttp` registers with `Isolation::Isolated`. Under `Isolated`, `fetch`
stores the *config plus a factory* and, the first time each core touches it, clones
the config, relocates it to that core, and runs the factory to build a fresh
transport instance for that core (cached per affinity). So each core gets its own
newly built `WinHttpTransport` with core-local pools (§5). (`Isolation::Shared` would
instead build one instance and share it across all cores.) We choose `Isolated` so
the `!Sync` `plurality` object pool (§5) can be core-local; the handler must still be
`Sync`, so that one pool sits behind a coarse `Mutex` (§5). `WinHttpDeps` derives
`ThreadAware` so `fetch` can clone and relocate the config per core.

The OS session - which owns session-scoped state, most importantly the connection
(keep-alive) pool - is opened by the factory when `fetch` materializes a per-core
transport instance, from the finalized `CustomContext`, not eagerly in `builder_winhttp`.
This is deliberate: `HttpClientBuilder` is `Clone`, so a session opened up front and
captured in the (clone-shared) factory closure would be shared by every client built from
that builder or any clone of it, letting two independently built clients reuse each
other's pooled connections - violating the pool-isolation boundary of design.md §2.
Opening it inside the factory (exactly as the Tokio transport builds its hyper client in
its own factory) scopes the session to the built client: two independently built clients -
including two builds of a cloned builder - never share a session or its pool.

Under `Isolation::Isolated` the factory runs once per core; `fetch` additionally invokes
it once per configured `multiple_pools` slot (`0..pool_count` in `client_builder.rs`, a
single slot by default), so each (core × pool slot) opens its own session and the client
holds one connection pool per (core × pool slot) rather than a single cross-core pool -
one pool per core in the default single-slot case. That is an acceptable, even preferable,
trade: core-local pools stay warm and
uncontended (see Future exploration below). A single session shared across a client's
cores *and* isolated between independently built clients is not expressible with today's
custom-transport API - it exposes only builder-scoped state (shared across clones) or
per-core/per-slot state (not shared across cores), with no per-built-client scope - so it
is noted as `fetch` API feedback (../../fetch/docs/stabilization.md, connection-management
item). Each per-core session is immutable after its setup, so a plain `Arc` (cloned into
that core's in-flight requests) suffices; the per-core object pool is the only mutable
shared state (`Mutex`-guarded, §5), while the event pool and read-buffer `GlobalPool` are
already thread-safe. All are uncontended under thread-per-core use.

**Contrast with `fetch_hyper`.** `fetch_hyper` uses `Isolation::Shared`: one hyper
client, already fully thread-safe, shared across cores, so its pool is process-wide by
construction. `fetch_winhttp` instead keeps a session (and therefore a connection pool)
per core, trading a single cross-core pool for warmer, core-local pools that need no
cross-core coordination.

**Future exploration.** The per-core session baseline is open to revision after
performance analysis: a future design could consolidate to one session shared across a
client's cores - recovering cross-core connection reuse and enabling session-granularity
connection recycling (the connection-lifetime control WinHTTP otherwise denies us,
design.md §2.2) - but doing so cleanly needs a `fetch` per-built-client shared-state hook
(or accepting `Isolation::Shared` at the cost of core-local object pools). It is not
obvious either beats per-core sessions with warmer, core-local pools, so v2 may revisit
it. Relatedly, whether per-core instancing is the right default at all could become a
knob: a low-traffic client has no need for per-core instances and might prefer a single
shared instance. This is left unconfigurable in v1 - a knob earns its place only with
demonstrated value, and per-core is a sound default - but is a candidate future
opportunity if profiling shows it matters.

Connection management (connect handles, reuse, lifetime) gets its own chapter (design.md §2).

### 3.3 Callback to future handoff via `events_once`

Each async WinHTTP step is a one-time signal from the callback (whether it fires
inline on the submitting thread or later on a WinHTTP worker thread, §3.1) to one
awaiting future, carrying a small payload. That is exactly `events_once`.

For each async step the `RequestDriver` (§4.4) rents a
`(sender, receiver)` pair from a transport-owned, per-core
`events_once::EventPool<CompletionResult>` (§5), stores the sender in the
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

### 3.4 Cross-thread handles

Raw WinHTTP handles are `*mut c_void` and thus neither `Send` nor `Sync`. They
are wrapped in `handle.rs` newtypes with explicit unsafe marker impls justified by
WinHTTP's documented cross-thread handle usability, mirroring the
`ThreadSafe<HANDLE>` technique in `oxidizer_io`. Three tiers, because their sharing
needs differ:

- **Request handles are `Send + Sync` behind the shared request state.** One send-only
  and one receive-only operation may use the same handle concurrently. The unsafe
  `Sync` implementation is limited to methods that preserve WinHTTP's documented
  directional concurrency rules and is covered by the full-duplex probe.
- **Connect handles are `Send` but not `Sync`.** They are retained for request lifetime
  but never used concurrently after request construction.
- **The session handle is `Send + Sync`.** A session `Arc` is cloned into every
  in-flight request on its core and is touched by WinHTTP's process-global callback
  threads (§3.1), so it is shared by reference across threads.
  The handler that holds it must be `Send + Sync` (a `fetch` requirement), so the session
  must be too. This is
  sound because WinHTTP explicitly permits concurrent operations on one session
  handle, and after its build-time setup the session is read-only from our side (§3.2);
  the `unsafe impl Sync for WinHttpSession` carries exactly that justification.

The future therefore holds only `Send` state (an `events_once` receiver plus
request/connect handle wrappers and a session `Arc`), satisfying `fetch`'s
`Out: Send` requirement.

## 4. Cancellation model and FFI ownership

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

### 4.1 Per-direction operation slots

WinHTTP allows one send-only and one receive-only operation to overlap on supported
systems, while still permitting only one outstanding operation within each direction.
`RequestContext` therefore contains independent send and receive slots plus shared
request-level state. WinHTTP delivers every completion for the handle through the same
callback context pointer; the completion status and failing API identify its lane.

The request handle lives in the driver (§4.4), not in this context: the callback
only recovers the context, takes the sender and buffer, and signals (§2.1), while
the driver uses the handle to issue the next call and, once, to close. That split
gives a single close authority (the driver's `RequestGuard`).

```rust,ignore
struct RequestContext {
    send: OperationSlot,
    receive: OperationSlot,
    secure_failure_flags: core::sync::atomic::AtomicU32,
}

enum OperationSlot {
    Idle,
    Active {
        completion: events_once::PooledSender<CompletionResult>,
        buffer: OperationBuffer,
    },
}

enum OperationBuffer {
    None,
    Read(bytesbuf::BytesBuf),
    Write(bytesbuf::BytesView),
}
```

Each lane moves independently between `Idle` and `Active`. Its active state carries
one completion sender and at most one borrowed buffer. A read and a write may therefore
borrow different buffers concurrently without aliasing. The implementation uses
separate interior-mutable cells with a one-driver/one-callback temporal ownership proof
per lane; callbacks never create a shared mutable reference spanning both slots.

### 4.2 dwContext is pointer-sized

WinHTTP stores the callback context as a `DWORD_PTR`, which is by definition
pointer-sized on Windows. `WINHTTP_OPTION_CONTEXT_VALUE` sets it by reading a
`*(DWORD_PTR*)`, and the callback receives that same value as its `dwContext`
parameter. A raw `*mut RequestContext` therefore round-trips through the context
value without truncation. This is the established production pattern for async
WinHTTP transports (store an owning per-request pointer in the context value and
reclaim it on `HANDLE_CLOSING`); we use the same shape with a pooled context
(§5) rather than an `Arc`.

### 4.3 Ownership rule: the pool owns the context, the callback frees it

A `RequestContext` is a `plurality::Box<RequestContext>` rented from a per-core
pool (§5) and handed to WinHTTP as the opaque handle context. One rule governs its
lifetime: **the driver owns the `Box` until `WinHttpSetOption(CONTEXT_VALUE)`
succeeds; after that WinHTTP owns it and the callback reclaims it on the final
`HANDLE_CLOSING`.**

The status callback is registered once on the session handle at build time (§2, step
S2) with mask `ALL_COMPLETIONS | SECURE_FAILURE | HANDLES`, and every request handle
inherits it. The only per-request handoff is installing the context pointer via
`WinHttpSetOption(WINHTTP_OPTION_CONTEXT_VALUE, ptr)`.

**Ordering:** the driver issues this `SetOption` *before* the first async call
(`WinHttpSendRequest`). This guards the window where an async call fails
synchronously: because the context is already installed, closing the handle still
delivers a `HANDLE_CLOSING` that carries the pointer, so the callback reclaims the
`Box` on the normal path instead of the driver needing a second, racy free path. Were
the context installed *after* the send, a synchronous send failure could close the
handle with a null context and strand the `Box`.

Before that `SetOption` succeeds, a failed `WinHttpOpenRequest` or `SetOption` lets
the driver drop the `Box` directly back into the pool. This is safe because WinHTTP
initializes a handle's context to null, and the trampoline ignores any callback
(including `HANDLE_CLOSING`) whose context is null - so early-failed requests,
connect handles, and the session never reconstruct a `Box`.

After `SetOption` succeeds, dropping the `RequestGuard` owner (the driver or the
`WinHttpBodyReader` it moves into, §6.3) synchronously closes the request and
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

### 4.4 Request setup splits into directional drivers

`RequestDriver` performs translation, handle setup, and `WinHttpSendRequest`. It initially
owns the `RequestGuard`: the request handle, connect handle, session
`Arc<WinHttpSession>`, and raw `RequestContext` pointer whose close path is described in
§4.3. After send completion it creates shared request state and starts an upload driver
and a response driver. Either may make progress independently; response headers can return
an `HttpResponse` while the upload driver still owns request body state.

The shared state, not either directional driver, owns the single close authority. The
context remains only the callback mailbox and contains no ownership of native handles.

### 4.5 Per-lane exclusive access without locks

Each slot's buffer and sender are shared between its driver and callback with no lock.
This is sound because access within that lane is strictly non-overlapping in time:

- **A directional driver populates its slot and drops every borrow before issuing
  the async call.** It moves that `OperationSlot` to `Active { .. }` (installing the
  completion sender and operation buffer) through the raw pointer, ends that borrow,
  *then* calls `Bindings::read_data` (etc.). It holds no
  `&mut RequestContext` across the submit boundary.
- From the submit call until the `events_once` receiver resolves, that driver touches
  nothing in its slot. WinHTTP holds its temporal ownership for the operation's duration
  (§4.2); the other lane remains independent.
- The **completion** for the operation - inline on the submitting thread, or later
  on a worker thread - is the sole accessor of the `Active` fields: it `take`s the
  sender and buffer (moving the context back to `Idle`) and sends. WinHTTP delivers
  exactly one completion per async operation, and each driver keeps one operation
  outstanding in its lane, so no second callback touches those fields. Send and receive
  callbacks may execute concurrently, but access disjoint slots. This exclusivity does
  **not** rely on undocumented callback serialization.
- The `events_once` send-then-receive is the release/acquire edge that transfers
  buffer ownership back to the driver. Only after the receiver resolves does the
  driver read the returned buffer.

This is the "WinHTTP takes exclusive ownership of one lane through the leaked context,
we recover it at the callback" model. No lock is needed on sender/buffer fields; each
lane's temporal hand-off and the structural separation between lanes do the work.

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
non-blocking (§2.1).

### 4.6 The outer connect-timeout race

The total connect deadline design.md §6.2 requires is the transport's *only*
self-scheduled timer; every other timeout is a native WinHTTP timer (§2.1). The
`RequestDriver` races the connect/send phase against a single
`tick::Clock::delay(connect_timeout)`, using the clock already threaded in from
`CustomContext` (no new dependency). Whichever finishes first wins: if the timer
fires, the driver closes the request handle - which cancels the in-flight connect
(§4.3) - and returns `HttpError::timeout`; if the connect completes first, the timer
future is dropped. `tick::Clock` is the sole source of time on this path; the
transport never calls `std::thread::sleep` or `tokio::time`.

## 5. Object and event pooling

Callbacks are hot and frequent and each request issues several async steps, so
the transport avoids per-step allocation by keeping two transport-owned pools that
requests rent from. Both must fit the `Send + Sync` bound a `fetch` handler
requires: the handler is stored in an `Arc<T>` shared into every request
future, and `Isolation::Isolated` gives each core its own instance but does not
relax that bound.

- **`events_once::EventPool<CompletionResult>`.** For each async step the driver
  (or the response body reader) rents one `(sender, receiver)` pair, and the callback
  sends the completion through it (§3.3). The pool is already `Send + Sync`, so it is
  held as a plain field, cloned into each request, and a callback may complete an
  event on any thread.
- **`plurality::Pool<RequestContext>` behind a `Mutex`.** A request rents one
  `plurality::Box<RequestContext>` at start and holds it for its whole lifetime
  (reclaimed when the callback drops it on `HANDLE_CLOSING`, §4.3). `plurality::Pool`
  is `Send + !Sync`, so it is the one field we wrap in a `std::sync::Mutex`. The lock
  is coarse but essentially uncontended: it is taken only to rent a context at request
  start - never across an `.await`, never by the body reader (which reuses the
  already-rented context), and never in a callback (the context returns itself to the
  pool through its own `Drop`, holding no `&Pool`).

Read buffers come from neither pool: they are reserved from the `bytesbuf`
`GlobalPool` on `CustomContext` (a `Sync`, `Arc`-backed pool), so the body reader
holds its own clone and rents buffers with no lock.

## 6. Request/response body streaming

Bodies are modeled as `bytesbuf_io` streams, then bridged to `fetch`'s
`http_body::Body` model. Response (read) buffers are drawn from the client's
`bytesbuf::mem::GlobalPool`. Request (write) buffers are whatever `BytesView` the
caller supplies in the outgoing `HttpBody`; they need not come from that pool.
Either way WinHTTP is agnostic to where the memory came from: it borrows the
pointer for the duration of one async operation (§4) and never allocates or
accounts for it, so the transport imposes no allocation-source requirement on
request bodies.

### 6.1 Outgoing request body -> `bytesbuf_io::Write`

`HttpRequest`'s body is an `HttpBody: http_body::Body<Data = BytesView>`
(pull-based `poll_frame`). The WinHTTP write side is a `bytesbuf_io::Write`:

```rust,ignore
impl bytesbuf_io::Write for WinHttpBodyWriter {
    type Error = HttpError;
    async fn write(&mut self, data: BytesView) -> Result<(), HttpError> {
        // Stores `data` in RequestContext.write_buffer, issues WinHttpWriteData,
        // and awaits WRITE_COMPLETE via an events_once step. The BytesView stays
        // pinned until the callback fires (§4).
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

### 6.2 Incoming response body <- `bytesbuf_io::Read`, then an `http_body::Body`

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

The resulting `HttpBody` is pull-based:
WinHTTP reads are issued lazily as the consumer polls, so backpressure is natural and
there is no unbounded buffering. The request's body idle timeout
(`http_extensions::BodyTimeout`) is enforced natively rather than by a Rust-side idle
wrapper: its reset-on-progress semantics match `WINHTTP_OPTION_RECEIVE_TIMEOUT`, so the
driver programs that native per-read timer from the request's `BodyTimeout` (§10.4),
keeping the body path free of a self-scheduled timer (the connect deadline stays the sole
exception, §4.6).

After the authoritative zero-length read, the body reader queries
`WINHTTP_QUERY_FLAG_TRAILERS`. A non-empty result becomes the terminal trailer frame for
HTTP/1.1, HTTP/2, or HTTP/3. The supported Windows baseline provides this query flag, so
HTTP/1.1 trailers are not discarded merely because older WinHTTP versions lacked the API.

The `READ_COMPLETE` buffer WinHTTP fills is a slice reserved inside a pooled
`BytesBuf`; it stays pinned until the callback fires (§4), then the filled prefix
is yielded as a zero-copy `BytesView`.

### 6.3 End-to-end request lifecycle (`RequestDriver` in `request.rs`)

`WinHttpTransport::execute(req)` returns a `Send` future implemented by
`RequestDriver`, which runs (each `->async` awaits an `events_once` completion;
§3.3):

```text
translate req (method/uri/headers -> UTF-16)
  -> open connect handle (inline WinHttpConnect; non-blocking, no cache, §9.1)
  -> WinHttpOpenRequest + set options (protocol, redirect, cookies/auth off, security, timeouts)
  -> set RequestContext pointer as WINHTTP_OPTION_CONTEXT_VALUE
  -> WinHttpSendRequest ->async SENDREQUEST_COMPLETE
  -> start independent send and receive lanes:
       send:    poll request data -> WinHttpWriteData ->async WRITE_COMPLETE [repeat]
       receive: WinHttpReceiveResponse ->async HEADERS_AVAILABLE
  -> WinHttpQueryHeaders (status, negotiated version, header block)  [sync]
  -> build HttpResponse { parts, duplex request lifetime, HttpBody streamed from WinHttpBodyReader }
  -> return Ok(response)   // upload may still be active while the caller reads the response
```

Header translation is mechanical: request headers serialize to a WinHTTP CRLF
header blob; the response `WINHTTP_QUERY_RAW_HEADERS_CRLF` blob parses back into
an `http::HeaderMap`. Method and URI come from the `http::Request` parts.

**Duplex handle ownership.** The request handle must outlive `execute` because response
body reads and request body writes may both continue after headers arrive. At that point
the `RequestGuard` moves into shared request state owned by the upload driver and
`WinHttpBodyReader`. It carries the request and connect handles, session
`Arc<WinHttpSession>`, context pointer, and single close authority. Holding the session
in every live request keeps it alive for as long as either direction needs it.

The shared state closes only after both directions finish, or immediately when either side
requests cancellation. A send failure before headers fails `execute`; a later send or trailer
failure is published to the response/request completion path. Dropping the response cancels an
unfinished upload. The close authority remains unique and closes exactly once.

## 7. Test plan

The bindings facade (§1) makes the transport testable at two levels. Unit tests
drive the `RequestDriver` against `MockBindings` with synthesized callbacks;
integration tests exercise the real OS against a localhost server. No test depends
on Tokio directly.

**Miri.** The FFI path cannot run under Miri, so the cancellation/leak invariants
(§4) are asserted on the mock path, which can, plus the real integration tests.
The mock path exercises the same allocate/leak/reclaim code as production so that
"free exactly once on `HANDLE_CLOSING`" is verified where Miri is available;
integration tests are `#[cfg_attr(miri, ignore)]`. Miri coverage is not a separate
concern from the test strategy but a property of it: the mock-bindings unit tests
are written so the ownership-critical paths run and are checked under Miri.

### 7.1 Unit tests (mock bindings, no network)

A `TestHarness` builds the transport over `MockBindings` and exposes
`complete(op, result)` to fire the callback the real trampoline would fire. This
deterministically interleaves completions and injects cancellations and
out-of-order callbacks.

### 7.2 How each key factor is tested

Most factors are checked by asserting the calls and values the driver hands the mock.
The three with nontrivial ordering - reentrant completion, cancellation / FFI
ownership, and body streaming - need scripted sequences and are described as prose
after the table.

| Factor | Key assertions | Notable adverse / edge case |
|--------|----------------|-----------------------------|
| Threading (§3) | completions fired from a foreign OS thread reach the awaiting future; `static_assertions` for `execute`'s future `Send`, request/session handles `Send + Sync`, connect handles `Send`+`!Sync`, handler `Send + Sync`, and per-core-owned pools | send and receive callbacks may overlap without sharing one operation slot |
| Error handling (design.md §7) | table-driven Win32/`WINHTTP_*` code -> `ErrorLabel` + `RecoveryInfo`; `GetLastError` mapping on a failing synchronous call | a 4xx/5xx response is `Ok`, not `Err` |
| Protocol negotiation (design.md §3) | portable HTTP/1.1/2 constraint filters WinHTTP's defaults and optional HTTP/3 preference; response `Version` comes from the queried negotiated protocol | exact HTTP/2 suppresses the HTTP/3 preference and sets `HTTP_PROTOCOL_REQUIRED` |
| TLS (design.md §4) | `WINHTTP_FLAG_SECURE` iff `https`; security-flags bitmask per `accept_invalid_*`, each flag setting only its own `SECURITY_FLAGS` bit (the two are independent, not coupled); `SECURE_FAILURE` -> `tls`-labeled, non-retryable | mTLS out of scope (design.md §4.1) - nothing to assert |
| Encoded responses / redirects / statelessness (design.md §5) | native decompression remains disabled; `REDIRECT_POLICY_NEVER`, `DISABLE_COOKIES`, and `DISABLE_AUTHENTICATION` are set; a 3xx is surfaced verbatim | encoded bytes and headers reach fetch-level decompression unchanged |
| Connection management (design.md §2) | connect handle opened per request and closed with it; max-conns mapping; `ConnectionKeepAlive` mapped to `HTTP2/3_KEEPALIVE` interval (§10.3); `DISABLE_GLOBAL_POOLING` on the session | `connection_lifetime` Fixed/PerConnection: accepted, no recycling, emits the `warn` "not honored" event; keep-alive `timeout`/active-only nuances emit the same warn |
| Timeouts (design.md §6) | `WinHttpSetTimeouts` gets resolve from `WinHttpOptions` and connect from `TransportOptions.connect_timeout` (the single connect-timeout source, §10.4); per-request `BodyTimeout` -> `WINHTTP_OPTION_RECEIVE_TIMEOUT` and `ResponseTimeout` -> backstop `RECEIVE_RESPONSE_TIMEOUT`, both read from request extensions; mock-clock connect deadline (design.md §6.2): advance past `connect_timeout` -> handle closed + `HttpError::timeout` | a connect completing first drops the timer unfired; a per-request `BodyTimeout` overrides the session default on the native receive timer |

- **Inline / reentrant completion.** Configure `MockBindings` so an async call
  (e.g. `read_data`) fires its completion *synchronously, inline, on the submitting
  thread* before returning - the reentrant case `ASSURED_NON_BLOCKING_CALLBACKS`
  permits (§2.1). Assert the driver still observes the result correctly (the
  `events_once` send lands before the receiver is awaited) and that no borrow of
  `RequestContext` is held across the submit (§4.5). Also assert the
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
  with the full notification mask (§4.3). (8) **Session lifetime:** drop the last
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
  `WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH` plus an explicit `Content-Length` (§6.1).

### 7.3 Integration tests (real WinHTTP, localhost)

Gated behind `#[cfg(windows)]` and `#[cfg_attr(miri, ignore)]`, against a
localhost server (a small `std::net`-based server, or `wiremock` as used
elsewhere in `fetch`). These validate the real OS path end to end:

The [`WINHTTP_OPTION_RESOLUTION_HOSTNAME` feasibility probe](resolution-hostname-experiment.md)
is an executable precursor to these tests. It verifies that the network destination, TLS server
name, and HTTP authority can be controlled independently against the real OS API. The authority
case must remain covered because Microsoft documents replacing a `Host` header but does not
explicitly guarantee its observed translation to HTTP/2 `:authority`.

- GET/POST with small and large bodies; response body correctness and size.
- Streaming upload (unknown length -> automatic protocol framing) and streaming download; assert
  incremental delivery, not just final bytes.
- Encoded gzip/deflate/Brotli/zstd responses and their original headers reach `fetch`
  unchanged; fetch-level tests cover uniform streaming decompression.
- Full-duplex required HTTP/2 follows the executable
  [full-duplex streaming probe](full-duplex-streaming-experiment.md), for both known-length
  and unknown-length automatic-chunking uploads. Assert response data arrives before the
  final request chunk and upload continues afterward.
- HTTP/1.1 response trailers are queried after EOF and surfaced alongside HTTP/2/3 trailers.
  A request body declaring trailers is rejected before `WinHttpSendRequest`.
- Redirects are never followed (`REDIRECT_POLICY_NEVER`, §5/§10.3): a request to a
  localhost endpoint returning a 302 whose `Location` points at a sentinel endpoint
  asserts the 3xx status and `Location` header are surfaced unchanged and that the
  sentinel endpoint is never hit (server-side hit counter stays zero). This proves the
  option overrides WinHTTP's default follow-redirects behavior and is set at the right
  handle scope.
- Cookies are never stored or replayed (`DISABLE_COOKIES`, §5/§10.3): a first response
  sets `Set-Cookie`; a second request to the same authority asserts no `Cookie` header is
  attached (verified server-side). This proves WinHTTP's default cookie jar is disabled at
  the correct handle scope.
- `https` against a localhost TLS server, exercising `accept_invalid_certs` and
  `accept_invalid_hostnames` as *independent* relaxations. A valid-cert case proves
  normal trust works. Against a server with an untrusted (self-signed) cert *and* a
  hostname mismatch, assert all four flag combinations: neither flag -> fails;
  `accept_invalid_certs` alone -> still fails on the hostname mismatch;
  `accept_invalid_hostnames` alone -> still fails on the untrusted cert; both -> succeeds.
  This proves the two Schannel ignore flags are not accidentally coupled and each leaves
  the other validation active. (Client-certificate/mTLS is out of scope for v1, design.md §4.1.)
- Pool isolation across clients: two `HttpClient`s built independently - including two
  builds of a *cloned* `builder_winhttp` builder - issue requests to the same authority;
  assert via server-side connection counting that they establish *separate* connections
  and never reuse each other's, proving the per-built-client session/pool boundary
  (design.md §2, §3.2).
- Pool isolation across slots within one client: a single `HttpClient` built with
  `multiple_pools(2)` routes requests through both pool slots to the same authority;
  assert via server-side connection counting that a connection opened for one slot is
  never reused by the other, proving each slot lands in its own session/pool even though
  the transport ignores the `PoolIndex` *value* (§8). Optionally pair with a mock-bindings
  assertion that `WinHttpOpen` runs once per slot (§3.2).
- HTTP/1.1 vs HTTP/2 negotiation against a server that supports both (wiremock and
  hyper-based localhost servers cover h1 and h2); assert the reported response
  `Version`.
- HTTP/3: wiremock and hyper localhost servers do not speak HTTP/3, so h3 is
  tested against a localhost QUIC server stood up with the `quinn` + `h3` crates
  (dev-dependencies) using a self-signed cert and `accept_invalid_certs`. Assert
  the negotiated `Version` is HTTP/3, and separately assert the "h3 required but
  QUIC unreachable" path yields the expected failure (`0x2EFE`/`0x2EFD`).
- Independent endpoint identity: send a request whose public authority and loopback dial target
  are `localhost:<port>` while the TLS server name is a distinct DNS name. Assert that SNI and
  certificate hostname validation use the TLS name, while HTTP/1.1 `Host` and HTTP/2 `:authority`
  retain `localhost:<port>`. Also retain the hostname-mismatch negative control.
- Connection reuse: two sequential requests to the same authority reuse the
  connection (observable via server-side connection counting).
- Small-write latency follows the calibrated
  [Nagle behavior experiment](nagle-behavior-experiment.md). Retain the raw Nagle-on and
  `TCP_NODELAY` controls so a Windows or network change cannot produce a false classification.
  This is regression evidence for the transport invariant, not proof of a WinHTTP socket option.
- Timeout configuration is validated only structurally (unit, §7.4). Integration
  tests set every timeout large enough that it can never fire during a healthy
  run, so a tripped timeout is always a real failure, never a timing race. No
  integration test asserts a timeout *firing* against a slow/black-hole endpoint,
  because that would depend on real wall-clock timing and be flaky.
- Real cancellation: drop an in-flight download future and assert clean teardown
  (no panic, no leak), the integration counterpart to the unit cancellation tests.
- Leak-freedom soak: a process-wide counter increments on every `RequestContext`
  allocation and decrements on every reclaim (the same counter the mock path asserts,
  compiled into the real path behind a test-only feature). A soak test runs a large
  batch mixing normal completion, timeout cancellation, and mid-flight future drops,
  then asserts the live-context count returns to its baseline. This catches a missed
  `HANDLE_CLOSING` free on the real OS path, which Miri cannot see (§7 preamble).

The full `fetch` pipeline (retry/breaker/telemetry) is validated by building an
`HttpClient` via `HttpClient::builder_winhttp(...)` and asserting a real request round-trips,
mirroring `fetch`'s existing `requests` integration test structure.

### 7.4 Timeout testing

Timeout *configuration* is asserted in unit tests: the mock bindings record the
`WinHttpSetTimeouts` arguments and the `WINHTTP_OPTION_CONNECT_TIMEOUT` /
`WINHTTP_OPTION_RECEIVE_RESPONSE_TIMEOUT` set-option calls, so a test asserts that
each `fetch` timeout option is translated into the correct WinHTTP timer value.

The one transport-scheduled timer - the outer connect timeout (design.md §6.2) - *is*
driven by `tick::Clock`, so it is unit-testable deterministically: with a mock
clock and mock bindings that never complete the connect, a test advances the clock
past `connect_timeout` and asserts the driver closes the handle and yields
`HttpError::timeout`, and conversely that a connect completing before the deadline
drops the timer without firing.

Timeout *firing against the real OS* cannot be made deterministic: the real
WinHTTP path uses the real OS clock, which the tests cannot freeze or
fast-forward. Real-time integration tests are therefore unacceptable (they would
be flaky). Integration tests instead configure timeouts large enough that they can
never fire during a healthy run, so a timeout tripping is always a genuine failure
signal, never a timing race. Verifying that a given deadline *does* fire is left
to the unit path (mock clock for the connect deadline; assert the option is set
for the native timers) rather than to any wall-clock integration test.

## 8. Client construction

The `HttpClientWinHttpExt` constructors `builder_winhttp`/`new_winhttp` (design.md §1.1)
do not reimplement any pipeline wiring; they delegate to `fetch`'s custom-transport
entry point, calling
`fetch::custom::create_builder("winhttp", "winhttp", factory, Isolation::Isolated, deps)`.
`new_winhttp` is just `builder_winhttp(WinHttpDeps::default()).build()`. The
`create_builder` signature this targets is:

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
placement comes from `Isolation::Isolated`, §3.2) and ignores `CustomContext::tls` (it
takes its own `WinHttpTlsConfig` instead; see design.md §1.2). Ignoring the `PoolIndex`
*value* does not collapse `fetch`'s `multiple_pools`: `fetch` invokes the factory once per
pool slot (`0..pool_count` in `client_builder.rs`), so each slot opens its own WinHTTP
session (§3.2), and because pooling is per-session (`DISABLE_GLOBAL_POOLING`, §9.3) those
sessions already hold distinct pools. Distinct `PoolIndex` slots therefore land in distinct
sessions/pools structurally, without the transport keying anything on the index. The real
v1 resource profile is one session/pool per (core × pool slot). Whether connection-pool
ownership belongs on `fetch` at all or entirely on the transport is unresolved and may
retire the `PoolIndex` surface in its current shape (../../fetch/docs/stabilization.md,
connection-management item).

`builder_winhttp` does **not** open the session; it just calls `create_builder` with the
factory. Each per-core transport instance opens its own session inside the factory when
`fetch` materializes it (§3.2), so the session is scoped to the built client and never
captured in the clone-shared builder closure. The session is deliberately not a
`WinHttpDeps` field either - `WinHttpDeps` stays plain, relocatable configuration. The
clock and read-buffer pool come
from `CustomContext`, so they are not duplicated in `Extras`. The `observed::Sink`
rides in `WinHttpDeps` and relocates per core with the rest of the config; the
transport emits its telemetry through it (detailed in v1.1). There is no
`anyspawn::Spawner`: no WinHTTP call the transport makes can block (§2.1).

## 9. Connection management internals

This chapter details how WinHTTP pools connections and how the transport configures it;
the caller-facing contract is in design.md §2.

### 9.1 Connect handles are logical, not connections

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
mutable state (a per-authority map, which would need
synchronization) to save a call that does no I/O, and it would buy no connection
reuse: reuse is keyed by authority in the session's pool and is independent of
which connect handle a request used. Dropping the cache keeps the transport's
shared state limited to the read-only session (§3.2).

### 9.2 HTTP/1.1 serialization and concurrency

For HTTP/1.1 there is no multiplexing: concurrent requests to the same authority
are serviced by separate pooled connections, bounded by
`WINHTTP_OPTION_MAX_CONNS_PER_SERVER`. WinHTTP performs this
serialization/pooling automatically; we do not manually serialize requests onto a
connection. HTTP/2 and HTTP/3 multiplex many requests over a single connection,
also handled by WinHTTP. Our only lever on concurrency is the max-connections
option (design.md §2.1).

### 9.3 Our responsibilities for reuse and draining

To get connection reuse we must: keep one session (§3.2) and not set
`WINHTTP_DISABLE_KEEP_ALIVE`. WinHTTP does the rest - reuse is session-level and
keyed by authority, independent of connect-handle identity (§9.1), so opening a
fresh connect handle per request costs nothing in reuse terms.

**We do set `WINHTTP_OPTION_DISABLE_GLOBAL_POOLING` on the session at creation.**
By default WinHTTP may share pooled connections process-wide across sessions. That
would let two independent `fetch_winhttp` clients - for instance a strict one and
one built with `accept_invalid_certs` (design.md §4) - reuse each other's pooled
connections, collapsing the security boundary between them. Disabling global
pooling scopes the connection pool to this session, so each `HttpClient` gets its
own pool (one pool per (core × pool slot) under the per-session model, §3.2)
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
covered in design.md §2.2.

**Deferred (v2): stale-connection retry.** WinHTTP offers
`WINHTTP_OPTION_FAILED_CONNECTION_RETRIES` scoped to
`WINHTTP_CONNECTION_RETRY_CONDITION_STALE_CONNECTION`, which makes WinHTTP
transparently reconnect and retry a request that lands on a pooled connection the
server already closed (safe even for non-idempotent requests, since a stale
connection means the request never reached the server). v1 does not use it - the
`fetch` pipeline's own retry covers the failure, if coarsely. It is recorded here as
a candidate to revisit alongside session lifecycle and recycling in the v2 "what do
we do about sessions" discussion.

## 10. WinHTTP request option mapping

How the contract in design.md §3-§6 is expressed to WinHTTP. None of this is part of the
public contract.

### 10.1 HTTP protocol flags

The resolved portable constraint and WinHTTP preference from design.md §3 map to request
options as follows.

- HTTP/1.1 is WinHTTP's baseline and is always available unless explicitly
  disallowed (below).
- HTTP/2 is enabled by
  `WinHttpSetOption(WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL, WINHTTP_PROTOCOL_FLAG_HTTP2)`.
- `prefer_http3` adds `WINHTTP_PROTOCOL_FLAG_HTTP3` only when the portable requirement
  leaves HTTP/3 available. It never sets a strict HTTP/3 requirement.

ALPN is performed by Schannel during the TLS handshake; there is no manual ALPN
wiring. The negotiated version is read back after `HEADERS_AVAILABLE` via
`WINHTTP_OPTION_HTTP_PROTOCOL_USED` and set on the `HttpResponse`, so upstream
telemetry reflects what was actually negotiated rather than what was requested.

**Resolved-set semantics:**

- Unspecified + default transport policy: enable HTTP/2 and allow HTTP/1.1 fallback.
- Unspecified + `prefer_http3`: enable HTTP/3 and HTTP/2, allowing HTTP/1.1 fallback.
- Exact HTTP/2: enable only HTTP/2 and set `WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED`.
- HTTP/1.1 or HTTP/2: enable HTTP/2 and allow HTTP/1.1 fallback; ignore `prefer_http3`.
- Exact HTTP/1.1: enable neither advanced protocol.

The portable configuration does not accept HTTP/3. A transport preference eliminated by an
explicit portable requirement is silently narrowed because satisfying requirements is the
documented precedence rule, not an option-honorability failure.

### 10.2 TLS flags

The `WinHttpTlsConfig` knobs from design.md §4 reach Schannel as follows. Knobs are
applied with `WinHttpSetOption` on the request handle before `WinHttpSendRequest`:

- **`https` selection.** `WINHTTP_FLAG_SECURE` on `WinHttpOpenRequest` for
  `https://` targets.
- **Insecure mode.** `accept_invalid_certs` / `accept_invalid_hostnames` set
  `WINHTTP_OPTION_SECURITY_FLAGS` with the relevant
  `SECURITY_FLAG_IGNORE_UNKNOWN_CA | IGNORE_CERT_CN_INVALID |
  IGNORE_CERT_DATE_INVALID | IGNORE_CERT_WRONG_USAGE` bits.
- **Server certificate inspection / pinning.** Not offered in v1. If needed later
  it hooks the `SECURE_FAILURE` callback and a post-handshake
  `WINHTTP_OPTION_SERVER_CERT_CONTEXT` query.
- **Client certificates (mTLS).** Out of scope for v1 (design.md §4.1). Wiring them
  into Schannel means importing a DER chain plus PKCS#8 key into an in-memory store,
  producing a `PCCERT_CONTEXT`, attaching it with
  `WINHTTP_OPTION_CLIENT_CERT_CONTEXT`, and managing hardware-backed identities.

### 10.3 WinHTTP-managed behavior flags

The behaviors in design.md §5 are configured through these options.

- **Automatic decompression remains disabled.** Do not set
  `WINHTTP_OPTION_DECOMPRESSION` and do not synthesize `Accept-Encoding`; the raw encoded
  body and headers must reach the mandatory fetch-level decompression layer.
- **Redirects.**
  `WINHTTP_OPTION_REDIRECT_POLICY = WINHTTP_OPTION_REDIRECT_POLICY_NEVER`, so
  redirect responses (3xx) are surfaced to the caller unchanged rather than
  followed.
- **Cookies and automatic authentication.** Each request sets
  `WINHTTP_OPTION_DISABLE_FEATURE` with `WINHTTP_DISABLE_COOKIES` (WinHTTP neither
  stores `Set-Cookie` nor auto-attaches `Cookie`) and
  `WINHTTP_OPTION_DISABLE_FEATURE` with `WINHTTP_DISABLE_AUTHENTICATION` (WinHTTP
  does not intercept 401/407 or attach credentials).
- **Keep-alive probes.** `fetch`'s `ConnectionKeepAlive` (design.md §2) maps to
  `WINHTTP_OPTION_HTTP2_KEEPALIVE` / `WINHTTP_OPTION_HTTP3_KEEPALIVE`, which make
  WinHTTP send an HTTP/2 or HTTP/3 PING once a connection has been idle for the
  configured interval (WinHTTP requires `>= 5000 ms`), keeping pooled connections warm
  past the server's idle-close. `Disabled` leaves the options unset. `interval` sets
  the option value; the fit is imperfect and the residue follows the §11 warn policy:
  WinHTTP PINGs idle pooled connections and has no separate "active-only" mode (so
  `ActiveConnections` and `ActiveAndIdleConnections` behave alike), it exposes a single
  interval and manages the probe-response timeout itself (so the `timeout` field is not
  separately honorable), and HTTP/1.1 has no application-level PING (so keep-alive there
  is plain TCP connection reuse, always on unless `WINHTTP_DISABLE_KEEP_ALIVE`, §9.3).

### 10.4 Timeout mapping and the native-timer backstop

The timeout contract in design.md §6 maps to WinHTTP timers as follows.

| `fetch` concept | Type / default | Where enforced | WinHTTP equivalent |
|-----------------|----------------|----------------|--------------------|
| Connect timeout | `TransportOptions.connect_timeout` (30 s) | This transport (`fetch` core does not wrap connect; `fetch_hyper` enforces it in its own connector) | `WINHTTP_OPTION_CONNECT_TIMEOUT`, applied to the send-time TCP/TLS handshake |
| Response timeout | `http_extensions::ResponseTimeout`, read per-request | Above transport, in `fetch::HttpClient::execute` (wraps the whole pipeline, maps to `HttpError::timeout`) | No matching native timer: `RECEIVE_RESPONSE_TIMEOUT` covers only the post-send wait for headers, not the connect+send+headers *total* `ResponseTimeout` promises. Set as a looser backstop only (below); not a faithful remap. |
| Body idle timeout | `http_extensions::BodyTimeout`, read per-request | This transport, natively | `WINHTTP_OPTION_RECEIVE_TIMEOUT` set per-request from the request's `BodyTimeout`: a per-receive-operation idle timer reset each read, which matches `BodyTimeout`'s reset-on-progress idle semantics. |
| Seatbelt request timeout | `seatbelt::TimeoutLayer` (30 s) | Above transport | n/a |
| Resolve timeout | (no distinct `fetch` concept; transport-specific by design, see the fetch API stabilization feedback, ../../fetch/docs/stabilization.md) | this transport | `WinHttpSetTimeouts` resolve field, set from `WinHttpOptions` |
| Send timeout | (not a distinct concept: the send-then-await-headers span is what `ResponseTimeout` governs) | above transport, via `ResponseTimeout` | `WinHttpSetTimeouts` send field left at a loose backstop; not driven by a dedicated `fetch` knob |

`ResponseTimeout` and `BodyTimeout` are read from each request's extensions
(`http_extensions::RequestExt`), not from session-global config, so per-request overrides
are honored. `WinHttpSetTimeouts(resolve, connect, send, receive)` sets the four base
timers; the per-request `WINHTTP_OPTION_RECEIVE_TIMEOUT` (body idle) and the backstop
`WINHTTP_OPTION_RECEIVE_RESPONSE_TIMEOUT` are set on the request handle, the latter forced
by WinHTTP to be at least the receive timeout. Each is a native WinHTTP timer, scheduled by
WinHTTP inside its own async machinery (§2.1); the transport's own connect deadline
(design.md §6.2) is the sole exception.

WinHTTP always applies its own receive timers - they have non-zero defaults that
cannot be disabled - so the transport sets them to the `fetch`-configured value
rather than leaving them at WinHTTP's defaults. This serves two purposes. First,
it keeps the two layers in agreement: without it, WinHTTP's default timer could
fire *before* the `fetch`-level timeout and surface as a raw WinHTTP error
instead of the canonical `HttpError::timeout`. Second, it is a liveness backstop.
The `fetch`-level response and body timeouts are futures driven by the caller's
async executor; if that executor stalls, those timeouts cannot fire. The native
timers run on WinHTTP's own threads, independent of the caller's executor, so a
hung network read is always aborted at the OS level and its socket released,
regardless of executor liveness. In the normal case the `fetch`-level timeout
still fires first and reports the canonical error; the native timer only bites
when the upper layer cannot.

### 10.5 Data-path tuning

The transport applies no socket-buffer or congestion controls. WinHTTP exposes no raw socket and no
equivalent to `SO_RCVBUF`, `SO_SNDBUF`, or per-socket initial-congestion-window selection. Similarly,
`WINHTTP_OPTION_HTTP2_RECEIVE_WINDOW` remains unset so WinHTTP owns the complete receive flow-control
policy rather than combining an application-selected stream window with an unknown OS update
strategy.

There is no Nagle option to set. Conformance comes from the calibrated small-write integration
experiment in §7.3, which verifies behavior equivalent to the TCP transports' `TCP_NODELAY`
invariant. Application read/write buffering remains an internal implementation choice and must not
be described as a substitute for kernel or HTTP/2 flow control.

## 11. Handling options the transport cannot honor

`fetch`'s options arrive through its generic configuration surface, and callers set
them transport-agnostically, so the transport routinely receives settings it cannot
faithfully honor on WinHTTP - a `connection_lifetime` of `Fixed`/`PerConnection`
(design.md §2.2), an idle timeout WinHTTP ignores (design.md §2.1), and so on.

The policy is uniform for all of them: at build time, for each configured option it
cannot faithfully honor, the transport emits a `warn`-severity `observed` event (which
also drives a telemetry counter) naming the option, then proceeds.

The two alternatives are both worse. Silently ignoring an option would let a caller
who configured, say, a bounded connection age for cert rotation believe a guarantee is
in force when it is not. Hard-erroring would break `fetch_winhttp` as a drop-in
transport, since the offending config arrives through the generic
`ConnectionPoolOptions`/`TlsOptions` surface and works fine on `fetch_hyper`. Warning
makes the gap visible in logs and telemetry without failing otherwise-valid clients.

These gaps are a symptom of `fetch`-level over-abstraction; the proper fix is
transport-level configuration (see the fetch API stabilization feedback,
../../fetch/docs/stabilization.md). Until then, the warning is the safety net.

## 12. Telemetry

The transport reports through the `observed::Sink` supplied in its dependencies
(design.md §1.2). Two kinds of signal are emitted, and the distinction is
deliberate:

- **Metrics** (counters) stay low-cardinality: request count, error count, and the
  "option not honored" counter (§11). No per-request or per-connection attribute is
  attached to a metric.
- **Log events** may carry richer, higher-cardinality context that is useful for
  diagnosing a single failure but would be spam as a metric dimension.

**Cold-connect error attribution (log only).** WinHTTP fires
`CONNECTING_TO_SERVER` when a request establishes a *new* physical connection
rather than reusing a pooled one. When a request then fails (connect timeout,
`REQUEST_ERROR`, `SECURE_FAILURE`), the transport annotates the failure's log event
with a marker that the failure occurred on a freshly-established connection, plus the
measured connect duration. This lets an operator distinguish "the server/pool is
unhealthy" from "cold-connection establishment is slow or failing" - the two have
different remediations. This attribution is attached only to the log event; it is
**not** promoted to a metric label, to keep connection-establishment noise out of the
metric cardinality.

## 13. Dependencies

Planned crate dependencies (all `default-features = false`, per workspace policy):

- `windows` `0.62.2` (added to `[workspace.dependencies]`; oxidizer currently
  vendors only `windows-sys`), features `Win32_Networking_WinHttp` and
  `Win32_Foundation`, target-gated via `[target.'cfg(windows)'.dependencies]`.
- `fetch`, `http_extensions`, `fetch_options`, `bytesbuf`, `bytesbuf_io` (whose
  `ReadExt::into_futures_stream` feeds the response body, §6.2), `thread_aware`,
  `tick`, `events_once`, `plurality`,
  `ohno`, `recoverable`, `http`, `http-body`, `observed` (telemetry events and
  metrics),
  `widestring` (UTF-16), `smallvec`. No `anyspawn`: nothing the transport calls
  can block (§2.1), so there is no blocking pool and no `Spawner`.
- `events_once` provides the reusable one-shot event pool
  ([folo-rs/folo](https://github.com/folo-rs/folo)); its `EventPool<T>` is
  `Send + Sync`, cheaply clonable, and returns rented endpoints to the pool on
  drop with no external access (§5).
- `plurality` provides the FFI raw-pointer round-trip (`Box::into_raw`/`from_raw`,
  §4.3).
- Dev: `mockall`, `static_assertions`, a localhost test server (`wiremock` or a
  hand-rolled `std::net` server), `quinn` + `h3` (localhost HTTP/3 server),
  `testing_aids`.

No Tokio in non-dev code.

## 14. Future opportunities

Design points deliberately deferred in v1, recorded here so they are revisited when
the `fetch` API or profiling data makes them actionable:

- **Consolidate per-core sessions into one session per client.** v1 opens one WinHTTP
  session (and therefore one connection pool) per core - and per `multiple_pools` slot
  within a core (§3.2) - because that is the only shape the
  current `fetch` custom-transport API expresses while still isolating independently built
  clients (§3.2). This diverges from a single-session-per-client model: it trades
  cross-core connection reuse and session-granularity connection recycling (the
  connection-lifetime control WinHTTP otherwise denies us, design.md §2.2) for warmer,
  uncontended core-local pools. Once `fetch` grows a per-built-client shared-state hook -
  or once profiling justifies `Isolation::Shared` despite core-local object pools -
  revisit whether one shared session per client is the better default. Tracked as `fetch`
  API feedback (../../fetch/docs/stabilization.md, connection-management item) so the
  divergence is not forgotten when that API becomes more expressive.
- **Per-core vs shared instancing as a knob.** Whether per-core instancing is the right
  default at all could become configurable: a low-traffic client gains nothing from
  per-core instances and might prefer a single shared instance (§3.2). Left unconfigurable
  in v1 - a knob earns its place only with demonstrated value - but a candidate if
  profiling shows it matters.
- **Session-keyed connection pools (`PoolIndex`).** v1 does not key anything on `fetch`'s
  `PoolIndex` value. It does not need to: `fetch` invokes the factory once per pool slot, so
  each slot already opens its own session/pool (§8), giving a resource profile of one
  session/pool per (core × pool slot) rather than a single collapsed OS pool. If pool
  ownership stays a transport concern after the v2 sessions/pools discussion, revisit
  whether to interpret `PoolIndex` explicitly (../../fetch/docs/stabilization.md,
  connection-management item).
