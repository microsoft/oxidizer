# `fetch_winhttp` implementation

This document describes the implementation strategy of the `fetch_winhttp` crate:
the OS bindings facade, the WinHTTP asynchronous model, the threading and
cancellation/FFI-ownership machinery, object pooling, body-streaming mechanics,
and the testing strategy. The higher-level architecture, behavior, and design tenets are
documented separately in [design.md](design.md).

## Architecture at a glance

This is the whole design in one picture; the numbered chapters below elaborate each part.

- **One OS session per (processor × pool slot), scoped to the built client.** Each materialized
  transport instance owns one session. Independently built clients, including builds from
  cloned builders, never share a session or connection pool; cloned `HttpClient` values
  share their original client's resources (§3.2).
- **One transport instance per (processor × pool slot).** `fetch` clones and relocates the
  transport per processor (`Isolation::Isolated`, §3.2), then materializes one factory result
  for each configured pool slot. Each instance owns its context pool (§5) and one session
  `Arc`.
- **One `RequestDriver` future per request** (§4.4). It owns a `RequestGuard`
  containing the request handle and rents a pooled `RequestContext` - the slot
  WinHTTP calls back into, which retains the connect handle and session owner
  through the request handle's final callback (§4.1).
- **WinHTTP drives the I/O on its own threads** (§3). The transport issues
  asynchronous calls and each one signals completion back to the awaiting future
  through an `events_once` one-shot (§3.3). A completion runs either inline on the
  submitting thread (keeping work on one processor) or on a WinHTTP worker thread.
- **No blocking pool and no Tokio.** Every setup call is synchronous but performs no
  I/O, so it runs inline on the executor; only WinHTTP's own async steps defer
  (§2.1).
- **Ownership across FFI is callback-driven** (§4). Dropping the guard closes the
  request handle synchronously, but the `RequestContext` and its retained parents
  are freed only on WinHTTP's final `HANDLE_CLOSING` callback, which guarantees no
  use-after-free or premature parent invalidation under cancellation.

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
pub(crate) unsafe trait Bindings: Send + Sync + 'static {
    unsafe fn open(&self, user_agent: &U16CStr, flags: u32) -> Result<RawHandle>;
    unsafe fn set_timeouts(&self, h: RawHandle, resolve: i32, connect: i32, send: i32, receive: i32) -> Result<()>;
    unsafe fn set_status_callback(&self, h: RawHandle, cb: StatusCallback, flags: u32) -> Result<()>;
    unsafe fn connect(&self, session: RawHandle, host: &U16CStr, port: u16) -> Result<RawHandle>;
    unsafe fn open_request(&self, connect: RawHandle, method: &U16CStr, path: &U16CStr, flags: u32) -> Result<RawHandle>;
    unsafe fn set_option(&self, h: RawHandle, option: u32, value: &[u8]) -> Result<()>;
    unsafe fn send_request(
        &self,
        h: RawHandle,
        headers: &U16CStr,
        total_len: u32,
        context: usize,
    ) -> Result<()>;
    unsafe fn write_data(&self, h: RawHandle, buf: Option<NonNull<u8>>, len: u32) -> Result<()>;
    unsafe fn receive_response(&self, h: RawHandle) -> Result<()>;
    unsafe fn query_headers(&self, h: RawHandle, level: u32, buffer: Option<NonNull<u8>>, len: &mut u32) -> Result<()>;
    unsafe fn query_option(&self, h: RawHandle, option: u32, buffer: Option<NonNull<u8>>, len: &mut u32) -> Result<()>;
    unsafe fn query_data_available(&self, h: RawHandle) -> Result<()>;
    unsafe fn read_data(&self, h: RawHandle, buf: NonNull<u8>, len: u32) -> Result<()>;
    unsafe fn close_handle(&self, h: RawHandle) -> Result<()>;
}
```

- **Production impl** (`RealBindings`) wraps the `windows`-crate calls one-to-one
  with `// SAFETY:` notes, like `oxidizer_io`'s build-target bindings. Every
  referenced symbol exists in `windows` `0.62.2`. `open` always uses
  `WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY`; proxy mode is not configurable in v1.
- **Test impl** is `mockall`'s generated `MockBindings`, wrapped in a
  `BindingsFacade` enum (`Real` / `Mock(Arc<MockBindings>)`), matching
  `oxidizer_io`'s bindings facade.
- The status callback cannot itself be a trait method (WinHTTP calls a bare
  `extern "system"` fn pointer). Tests therefore synthesize callbacks by invoking the
  crate-internal `dispatch_completion(context, status, status_info, status_info_len)`
  directly. That entry point is a plain fn precisely because it needs no captured state: all
  per-request state is reached through the `context` pointer (the `*mut
  RequestContext`, §4.2), and all recording/expectation state lives in the
  `Arc<MockBindings>` the harness owns via the `BindingsFacade`. Nothing is a global
  singleton. See §7.

**Safety contract of the `Bindings` API.** Because callers drive raw OS handles and
async buffer lifetimes through this trait, a small set of caller-side invariants
must hold for every impl (production or mock) to be sound. They are stated once here
and relied on throughout §4 and §6:

- A buffer handed to `write_data` must contain at least the submitted number of
  initialized readable bytes. A buffer handed to `read_data` must expose at least
  the submitted number of writable bytes. Both must stay valid and untouched until
  `WRITE_COMPLETE`/`READ_COMPLETE`, `REQUEST_ERROR`, or the request handle's final
  `HANDLE_CLOSING` callback terminates that operation (WinHTTP borrows them
  asynchronously). `send_request` always passes a null optional buffer; request
  bodies use sequential `write_data` calls.
- The `RequestContext` must be fully populated before the async call is issued, and no
  exclusive borrow of it may be outstanding across the submission, so that a possibly
  reentrant completion (§2.1) can take its own shared borrow and atomically claim the
  operation payload. Shared borrows may be held across a submission (§4.5).
- Every session is opened with `WINHTTP_FLAG_ASYNC`, and every child handle
  inherits the asynchronous callback behavior required by operations that omit
  synchronous output pointers.
- At most one async operation is outstanding per request handle at a time.
- The status callback must be registered with every completion, request-error,
  diagnostic, and final handle-closing notification consumed by the callback
  protocol. The context must be installed before the first async call, the
  `send_request` context value must exactly match that installed pointer, and each
  handle is closed exactly once (§4.3).

### 1.1 Crate/module layout

```text
crates/fetch_winhttp/
  src/
    lib.rs               // #![cfg(windows)] gate + module declarations + re-exports + crate docs
    builder.rs           // WinHttpDeps/WinHttpDepsBuilder and client-builder integration
    transport.rs         // WinHttpTransport: per-(processor × pool-slot) RequestHandler (§3.2)
    session.rs           // WinHttpSession: per-(processor × pool-slot) session handle (§3.2)
    request.rs           // RequestDriver: drives one request/response lifecycle (§6.3)
    context.rs           // RequestContext: pinned per-request state the callback reads
    callback.rs          // extern "system" trampoline -> dispatch_completion
    operation.rs         // ContextPool/ContextInstallation/RawContextOwner/RequestGuard/
                         //   OperationFuture: context installation and per-operation
                         //   handle ownership (§4.1, §4.3)
    query.rs             // QueryError + the synchronous WinHttpQueryOption/QueryHeaders
                         //   wrapper layer (§2.1)
    response_headers.rs  // pure &[u8] status-line/header/trailer parser (no WinHTTP calls)
    convert.rs           // ConversionError + numeric/duration/UTF-16/option-value
                         //   conversions, the unlimited-timeout sentinel, keep-alive floors
    body/
      read.rs            // response reader over WinHttpReadData + http_body adapter
      write.rs           // bytesbuf_io::Write over WinHttpWriteData + request framing
      mod.rs             // module wiring only (no type definitions)
    tls.rs               // WinHttpTlsConfig -> security flags
    options.rs           // The validated ProtocolOptions
    telemetry.rs         // observed::Sink metrics and log events (§12)
    handle.rs            // RAII handle wrappers (Send/Sync assertions)
    error.rs             // Win32 -> HttpError mapping + the shared error constructors
    error_labels.rs      // ErrorLabel constants
    testing.rs           // #[cfg(test)] mock-bindings harness shared by several modules
    bindings/
      abstractions.rs    // Bindings trait (OS entry-point contract)
      facade.rs          // BindingsFacade enum (Real / Mock dispatch)
      real.rs            // windows-crate impl (cfg(windows))
      mod.rs             // module wiring + the SDK constant re-export hub
  docs/design.md
  docs/implementation.md
```

The crate root is gated by `#![cfg(windows)]`, so it compiles to an empty module
on non-Windows targets. A non-Windows integration test keeps package-scoped test
runs nonempty, while target-specific dependencies ensure those builds do not pull
in the `windows` crate.

`bindings/mod.rs` re-exports the WinHTTP flag, option, and query constants used
across the crate, so an import path names the FFI boundary a value comes from; a
module imports a constant straight from the generated `windows` bindings when it
is local to that module's own concern, as `error.rs` does for the Win32 and
Winsock codes it maps. No module duplicates an SDK numeric value. The only local
numeric policy constants are the transport-level rounding minima and timeout
sentinel in `convert.rs` and the documented `HRESULT_FROM_WIN32` extraction masks
in `error.rs`.

Conversion failures use one `ohno` source type per condition and are wrapped in
`ConversionError` through generated `From` implementations. `QueryError` remains
the routing boundary between a WinHTTP operation failure and malformed data
returned by a successful query because callers map those categories to different
`HttpError` classifications.

### 1.2 Integration-test layout

The integration tests under `crates/fetch_winhttp/tests/` are split by concern
rather than by protocol version, so one binary owns one contract area:

```text
crates/fetch_winhttp/tests/
  protocols.rs         // negotiated-version reporting and per-protocol round trips
                       //   (HTTP/1.1, HTTP/2, HTTP/3), including required-h3 failure
  tls.rs               // the certificate-validation relaxation matrix
  transport_policy.rs  // request framing, trailer rejection, decoding, redirects,
                       //   cookies, authentication challenges
  lifecycle.rs         // pool isolation and reuse, cancellation, body drop, leak soak,
                       //   full fetch pipeline construction
  non_windows.rs       // keeps package-scoped test runs nonempty off Windows

crates/fetch_winhttp_testing/src/
  lib.rs               // shared client construction and frame-collection helpers
  server.rs            // TestServer: localhost TCP fixture (HTTP/1.1 and HTTP/2, TLS)
  http3_server.rs      // Http3Server: localhost QUIC fixture (HTTP/3)
  recording.rs         // ResponsePlan/RecordedRequest scripting and observation vocabulary
```

`fetch_winhttp_testing` is an unpublished package rather than a `tests/common`
module, so the integration tests, the examples, and the benchmarks all drive the
same fixtures. It also keeps the server ecosystem's dependencies - Tokio, hyper,
quinn, rustls - out of the transport package entirely.

`lifecycle.rs` is the only binary that builds a client through `fetch`'s standard
pipeline and therefore the only one whose requests pass through `fetch`'s logging
handler. An integration binary links the library with `cfg(test)` false, so no
crate-root initialization runs and that binary invokes
`testing_aids::init_tracing!()` at module scope itself
(../../../docs/tracing-tests.md).

### 1.3 Examples

`crates/fetch_winhttp/examples/` holds one runnable example per feature area, each
serving a localhost fixture from `fetch_winhttp_testing` so no example depends on an
external endpoint or on real-time waiting:

```text
crates/fetch_winhttp/examples/
  quick_start.rs        // builder_winhttp and the mandatory WinHttpDeps environment
  streaming_upload.rs   // unknown-length uploads and request-trailer rejection
  streaming_download.rs // frame-by-frame bodies, response trailers, mid-stream drop
  tls_validation.rs     // the strict default and the two independent relaxations
  http3.rs              // required HTTP/3 with no TCP fallback
  connection_pools.rs   // client clone, builder clone, and multiple_pools isolation
```

Each example is a `fn main` shim that delegates to a `#[cfg(windows)]` module and
prints an explanatory line elsewhere, so `cargo check --all-targets` stays green on
non-Windows targets where the crate itself is empty. They drive futures with
`futures::executor::block_on` rather than a runtime macro, which doubles as a
demonstration that the transport is runtime-neutral.

`fetch_winhttp_testing` is unpublished, so the examples ship in the package as
readable documentation but are not built by `cargo package`'s verification step or
by downstream consumers.

## 2. WinHTTP asynchronous model primer

A single request drives this WinHTTP handle chain and callback sequence. `S` steps
configure the session once when the transport instance is built; `R` steps run once
per request.

| Step | Call | Sync/async | Completion callback |
|------|------|-----------|---------------------|
| S1 | `WinHttpOpen(WINHTTP_FLAG_ASYNC)` | sync | - (build-time) |
| S2 | `WinHttpSetTimeouts`, session-scoped pool/callback/keep-alive options, `WinHttpSetStatusCallback` | sync | - (build-time; mask in §4.3) |
| R1 | `WinHttpConnect` | sync, inline (see §2.1) | - |
| R2 | `WinHttpOpenRequest` | sync | - |
| R3 | `WinHttpSetOption`xN (including context and request behavior) | sync | - |
| R4 | `WinHttpSendRequest` | async | `SENDREQUEST_COMPLETE` |
| R5 | `WinHttpWriteData` (streaming body, per chunk) | async | `WRITE_COMPLETE` |
| R6 | `WinHttpReceiveResponse` | async | `HEADERS_AVAILABLE` |
| R7 | `WinHttpQueryHeaders` | sync (buffered) | - |
| R8 | `WinHttpQueryDataAvailable` | async | `DATA_AVAILABLE` (n bytes) |
| R9 | `WinHttpReadData` | async | `READ_COMPLETE` (n bytes) then loop R8/R9 until 0 |
| R10 | `WinHttpCloseHandle` | sync | `HANDLE_CLOSING` (final callback) |

Errors on any async step arrive as `REQUEST_ERROR` carrying a
`WINHTTP_ASYNC_RESULT { dwResult, dwError }`. TLS validation problems may also raise
`SECURE_FAILURE`. No correctness or classification logic depends on those
notifications arriving in a particular order.

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
briefly block. Since each materialized transport instance opens its own session lazily
inside the factory when `fetch` materializes it (§3.2), this is a one-time
per-processor construction cost off the request path; the process-wide global
initialization runs only on the first such open.

The session sets `WINHTTP_OPTION_ASSURED_NON_BLOCKING_CALLBACKS`: we promise our
completion callbacks never block, and in return WinHTTP may invoke a callback
**inline** on the submitting thread whenever an operation completes immediately
(e.g. a read served from an internal buffer), instead of hopping to a thread-pool
worker. We want this - it removes a thread-pool hop on the hot path (§3.1).

The callback trampoline (§4) is safe to run reentrantly because it does a small,
bounded, non-blocking amount of work: recover the `*mut RequestContext`, take the
in-flight `events_once` sender and buffer, and send the `CompletionResult`. It
performs no I/O and never waits on WinHTTP. Each completion event is embedded in
the already-pinned request context, so cancellation never returns an event endpoint
through a mutex-protected pool on the callback thread. Returning the context `Box`
or a `BytesBuf` on `HANDLE_CLOSING` is likewise non-blocking. The one heavier case -
the last context `Box` drop freeing the `plurality` pool's chunks - happens only at
shutdown, where cost is irrelevant and only correctness matters; it is a heap free,
not a wait on WinHTTP, so the assurance still holds.

Reclaiming a context on `HANDLE_CLOSING` drops the handle owners it held, so
`WinHttpCloseHandle` runs on the callback thread for the request's parent
handles: always the per-request connect handle, and the session handle when that
context held the last reference to it. This keeps the assurance because
`WinHttpCloseHandle` only initiates teardown and returns without waiting for it,
and because the handle being closed is never the one whose callback is running -
the notifying request handle is already closing, and its parents are reachable
from no other in-flight call. WinHTTP requires only that no other thread is
calling WinHTTP with a handle while it is closed, which the per-request ownership
of the connect handle and the reference counting of the session handle both
establish (see
[Concurrency in WinHTTP](https://learn.microsoft.com/en-us/windows/win32/winhttp/programming-considerations#concurrency-in-winhttp)).

Reentrancy is sound because of one submitting-side rule (§4.5): the driver fully
populates the `RequestContext` before issuing the async call and holds no exclusive
borrow of it across the submission. However the completion then arrives, it atomically
claims only the active operation payload through the shared context pointer, and the
`events_once` send is the release/acquire edge that hands buffer ownership back.

## 3. Threading model

The design is about moving data safely between the kinds of threads in play:

1. **Async executor threads** (the caller's runtime). `WinHttpTransport::execute`
   runs here and the returned future is polled here. Every setup call (§2.1) runs
   inline on these threads; `fetch` requires the returned future to be `Send`.
2. **WinHTTP's own worker threads.** When a completion cannot be produced
   immediately, WinHTTP delivers it later on a thread it owns. The application
   neither creates, sizes, nor owns these threads.

A completion is delivered inline on the submitting executor thread or later on a
WinHTTP worker, per §2.1; the callback trampoline (§4) is identical either way, and
there is no additional "blocking pool" tier - no request-path call can block.

### 3.1 How WinHTTP schedules callback work

WinHTTP does not run a bespoke thread pool; it posts async completions to the
process-global Win32 thread pool, from which a worker dispatches our callback. There
is no per-request or per-handle thread affinity: successive completions for one
request can land on different workers, so no callback may assume it runs on the
thread that submitted the operation or on the same worker as the previous completion.
Soundness rests on documented properties: "exactly one completion per async
operation", "one operation outstanding per handle", and "`HANDLE_CLOSING` is the final
notification for a handle and does not overlap another callback for that handle"
(§4.5). The remaining completion-versus-synchronous-failure race is closed by an
atomic (§4.5).

The consequences shape the design: all per-request callback state must be reachable
from the context pointer alone and safe to touch from any thread (§4.1, with `Send`
handle wrappers, §3.4); and the callback-to-future handoff must be a real
cross-thread signal, not a shared cell - that is `events_once` (§3.3), which behaves
identically same-thread or cross-thread.

Inline completions (§2.1) give a degree of processor affinity for free: when an
operation completes immediately, WinHTTP runs the callback on the very thread that
submitted it, so completion work stays on the same processor as the async work that
issued it rather than hopping to an arbitrary thread-pool worker. Only genuinely
deferred completions incur the hop.

### 3.2 Per-processor/per-pool-slot transport instances and sessions

`fetch_winhttp` registers with `Isolation::Isolated`. Under `Isolated`, `fetch`
stores the *config plus a factory* and, the first time each processor touches
it, clones and relocates the configuration to that processor. Within that
processor it invokes the factory once for every configured `multiple_pools`
slot, caching each resulting `WinHttpTransport` separately. Each
(processor × pool slot) therefore has its own transport, session, and context
pool (§5). (`Isolation::Shared` would instead share materialized handlers
across processors.)

`Isolated` is a design principle rather than a workaround: thread-isolated
execution is the default because minimizing sharing is more efficient. That the
`!Sync` `plurality` context pool (§5) can stay instance-local follows from the
choice rather than motivating it. The handler must still be `Sync`, so that pool
sits behind a coarse `Mutex` (§5). `WinHttpDeps` derives `ThreadAware` so `fetch`
can clone and relocate the configuration per processor.

The OS session - which owns session-scoped state, most importantly the connection
(keep-alive) pool - is opened by the factory when `fetch` materializes a per-processor
transport instance, from the finalized `CustomContext`, not eagerly in `builder_winhttp`.
This is deliberate: `HttpClientBuilder` is `Clone`, so a session opened up front and
captured in the (clone-shared) factory closure would be shared by every client built from
that builder or any clone of it, letting two independently built clients reuse each
other's pooled connections - violating the pool-isolation boundary of design.md §2.
Opening it inside the factory (exactly as the Tokio transport builds its hyper client in
its own factory) scopes the session to the built client: two independently built clients -
including two builds of a cloned builder - never share a session or its pool.

Under `Isolation::Isolated` the factory runs once per processor; `fetch`
additionally invokes it once per configured `multiple_pools` slot
(`0..pool_count` in `client_builder.rs`, a single slot by default), so each
(processor × pool slot) opens its own session and the client holds one
connection pool per (processor × pool slot) rather than a single
cross-processor pool - one pool per processor in the default single-slot case.
That is an acceptable, even preferable, trade: processor-local pools stay warm
and uncontended (see Future exploration below). A single session shared across a
client's processors *and* isolated between independently built clients is not
expressible with today's custom-transport API - it exposes only builder-scoped
state (shared across clones) or per-processor/per-slot state (not shared across
processors), with no per-built-client scope - so it is noted as `fetch` API
feedback (../../fetch/docs/stabilization.md, connection-management item). Each
instance's session is immutable after setup, so a plain `Arc` cloned into that
instance's in-flight requests suffices. The instance-local context pool is the
only mutable shared state (`Mutex`-guarded, §5), while the read-buffer
`GlobalPool` is already thread-safe. All are normally uncontended under
thread-isolated use.

**Contrast with `fetch_hyper`.** `fetch_hyper` uses `Isolation::Shared`: one
hyper client, already fully thread-safe, shared across processors, so its pool is
process-wide by construction. `fetch_winhttp` instead keeps a session (and
therefore a connection pool) per processor, trading a single cross-processor pool
for warmer, processor-local pools that need no cross-processor coordination.

**Future exploration.** The per-processor session baseline is open to revision
after performance analysis: a future design could consolidate to one session
shared across a client's processors - recovering cross-processor connection reuse
and enabling session-granularity connection recycling (the connection-lifetime
control the transport does not offer today, design.md §2.2) - but doing so
cleanly needs a `fetch` per-built-client shared-state hook (or accepting
`Isolation::Shared` at the cost of processor-local object pools). It is not
obvious either beats per-processor sessions with warmer, processor-local pools,
so v2 may revisit it. Relatedly, whether per-processor instancing is the right
default at all could become a knob: a low-traffic client has no need for
per-processor instances and might prefer a single shared instance. This is left
unconfigurable in v1 - a knob earns its place only with demonstrated value, and
per-processor is a sound default - but is a candidate future opportunity if
profiling shows it matters.

Connection management (connect handles, reuse, lifetime) gets its own chapter (design.md §2).

### 3.3 Callback to future handoff via `events_once`

Each async WinHTTP step is a one-time signal from the callback (whether it fires
inline on the submitting thread or later on a WinHTTP worker thread, §3.1) to one
awaiting future, carrying a small payload. That is exactly `events_once`.

For each async step the `RequestDriver` (§4.4) places an `events_once` event in
the pinned request context, stores its sender in the active-operation slot, issues
the async call through `Bindings`, and awaits the receiver. The receiver
exclusively borrows the `RequestGuard`. This is the invariant the design defines:
another operation cannot reuse the embedded storage
until the event has reached its terminal state: either the value was received or
the receiver was destroyed after the sender disconnected. The sender may still be
returning from its send at that point, because `events_once` permits the storage
to be reused once the value has been delivered. When WinHTTP later invokes the
callback trampoline,
the trampoline reconstructs the `RequestContext` from the context value, takes the
stored sender, builds a `CompletionResult`, and sends it. The executor wakes and
the driver advances to the next state.

```rust,ignore
enum CompletionResult {
    SendRequestComplete,
    WriteComplete { buffer: bytesbuf::BytesView, len: u32 },
    HeadersAvailable,
    DataAvailable(u32),
    // Ownership of the read buffer is returned to the future here. `len` is the
    // number of bytes WinHTTP appended (metadata; the buffer may have carried
    // earlier bytes, since a BytesBuf need not be empty to be appended to).
    ReadComplete { buffer: bytesbuf::BytesBuf, len: u32 },
    Error {
        error: WinHttpError,
        _buffer: Option<CompletionBuffer>,
    },
    InvalidStatusInfo {
        status: u32,
        len: u32,
        _buffer: Option<CompletionBuffer>,
    },
}
```

The `_buffer` fields keep callback-owned read or write storage alive until the
driver consumes the completion payload. On `READ_COMPLETE` the driver needs the
buffer back, because it holds the bytes just received. On write and error
completions the buffer carries nothing the driver needs; it is retained purely so
that the trampoline does not run `BytesView`/`BytesBuf` destructors, and with them
a `GlobalPool` block release, on a WinHTTP callback thread. Deallocation happens on
the driver's own thread instead.

`events_once` is the right primitive because each step is a single, non-blocking,
one-shot, payload-carrying signal with exactly one waiter.

### 3.4 `Send` (not `Sync`) across the FFI boundary

Raw WinHTTP handles are `*mut c_void` and thus neither `Send` nor `Sync`. The explicit
unsafe markers live on exactly one type, the `RawHandle` newtype in `handle.rs`, justified
by WinHTTP's documented cross-thread handle usability and mirroring the
`ThreadSafe<HANDLE>` technique in `oxidizer_io`. Every owning wrapper is built from that
token, so no wrapper repeats the assertion; a wrapper instead *withdraws* what it must not
offer, by holding a `PhantomData<Cell<()>>` that removes `Sync`. The tiers differ because
their sharing needs differ:

- **Request and connect handles are `Send` but not `Sync`.** Each belongs to one
  request; the handle is only ever *moved* between threads (the future migrates
  across executor threads, and a completion may arrive on a different thread than
  the submit), never shared by reference from two threads at once. The driver keeps
  at most one operation outstanding per handle and holds the only reference, so
  `Send` alone is what we need, and the `not_sync` marker on `ConnectHandle` and
  `RequestHandle` is what holds them to it.
- **The session handle is `Send + Sync`.** A session `Arc` is cloned into every
  in-flight request on its processor and is touched by WinHTTP's process-global callback
  threads (§3.1), so it is shared by reference across threads.
  The handler that holds it must be `Send + Sync` (a `fetch` requirement), so the session
  must be too. `SessionHandle` and `WinHttpSession` reach that by omitting the `not_sync`
  marker rather than by asserting anything further, which is sound because WinHTTP
  explicitly permits concurrent operations on one session handle and because the session
  is read-only from our side after its build-time setup (§3.2).

`RequestDriver` therefore holds only `Send` state: before context installation, the
request/connect wrappers and session owner; afterward, an `events_once` receiver
and the request guard, while the context owns the parent handles. This satisfies
`fetch`'s `Out: Send` requirement for the future that drives it.

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

### 4.1 The per-request operation slot

WinHTTP allows at most one outstanding async operation per request handle at a
time, and it delivers every completion for a handle to the same callback context
pointer. `RequestContext` contains an operation slot plus the parent handles whose
lifetime must extend through the request handle's final callback. The operation slot
is reused across the request's sequence of sequential operations (send, each request
write, receive, then each response read) instead of being reallocated per step. Its
pointer is what we hand to WinHTTP as the callback context; WinHTTP echoes it back on
every notification for that request handle.

The request handle lives in the driver (§4.4), not in this context: the callback
only recovers the context, takes the sender and buffer, and signals (§2.1), while
the driver uses the handle to issue the next call and, once, to close. The connect
handle and session owner move into the context before the context pointer is handed
to WinHTTP, so closing the
request cannot invalidate its parents while WinHTTP is still tearing it down.
WinHTTP specifies that closing a handle invalidates its children, so a connect or
session handle must outlive every request opened from it
([`WinHttpCloseHandle`](https://learn.microsoft.com/en-us/windows/win32/api/winhttp/nf-winhttp-winhttpclosehandle)).

```rust,ignore
struct RequestContext {
    // Reused storage through which the callback hands completions back to the
    // driver. The request handle itself is not here: OperationFuture owns it
    // until its receiver endpoint is destroyed.
    operation: CallbackOperationSlot,
    // Parent handles, retained until HANDLE_CLOSING drops the context. They must
    // outlive the request because Microsoft documents that closing a parent
    // invalidates its children and that pending child operations cannot then be
    // relied on to complete correctly.
    connect: ConnectHandle,
    session: std::sync::Arc<WinHttpSession>,
    // Request-scoped, best-effort diagnostics independent of operation state.
    // The REQUEST_ERROR code, not callback order, determines classification.
    secure_failure: SecureFailureDiagnostics,
    cold_connect: ColdConnectDiagnostics,
}

struct ActiveOperation {
    kind: OperationKind,
    // Completion sender for the in-flight operation; the callback takes it.
    completion: events_once::RawSender<CompletionResult>,
    // The buffer this operation borrows (if any). Read ops borrow a mutable
    // BytesBuf (WinHTTP appends response bytes); write ops borrow an immutable
    // BytesView (WinHTTP reads request bytes); send/receive/query ops borrow
    // none. Ownership passes to WinHTTP for the operation's duration (§4).
    buffer: OperationBuffer,
}

enum OperationBuffer {
    None,
    Read {
        buffer: bytesbuf::BytesBuf,
        address: usize,
        capacity: u32,
    },
    Write {
        buffer: bytesbuf::BytesView,
        len: u32,
    },
}

struct CallbackOperationSlot {
    // Idle, claimed, or an active OperationKind discriminant. This publishes
    // and arbitrates payload ownership; it does not validate API sequencing.
    state: core::sync::atomic::AtomicU8,
    active: core::cell::UnsafeCell<core::mem::MaybeUninit<ActiveOperation>>,
    // Pinned in RequestContext and reused only after the previous event reached
    // its terminal state: value received, or receiver destroyed after the sender
    // disconnected. events_once permits reuse from that point, so the previous
    // sender may still be returning from its send.
    completion: core::cell::UnsafeCell<events_once::EmbeddedEvent<CompletionResult>>,
}
```

The two diagnostic fields are dedicated types rather than bare atomics, so the
encoding stays with the data instead of being reconstructed at each use site.
`SecureFailureDiagnostics` wraps an `AtomicU64` and exposes `record(flags)` /
`observed() -> Option<u32>`; it packs the certificate-error flags in the low 32 bits
and a presence marker above them, which is what makes a zero-valued flag set
distinguishable from "no callback arrived". `ColdConnectDiagnostics` wraps an
`AtomicU8` holding a `ColdConnectState` discriminant and exposes typed transitions
and a typed read. Both keep `RequestContext` free of bit arithmetic.

The active operation makes the field relationships explicit: it always carries a
completion sender and at most one borrowed buffer (a handle never has a read and a write
outstanding at once); the idle state carries neither. Sequential submission is
guaranteed by construction: `OperationFuture` exclusively borrows `RequestGuard` and
moves the request handle out of it. Safe code therefore has no request handle
with which to arm another operation until the current receiver is destroyed and
completion restores the handle. Forgetting the future leaves the handle leaked
inside it rather than making the guard reusable. A debug assertion checks that
invariant when the slot is armed.

The atomic state has a separate production responsibility. It publishes the
initialized payload to callback threads and lets exactly one of a completion
callback, a synchronous submission failure, or final handle closure claim and
move out the sender and buffer. Competing or late callbacks fail that claim and
do nothing. The `UnsafeCell` is therefore not unsynchronized mutation: the atomic
tag grants one claimant access to the initialized contents. Secure-failure flags
are kept outside the slot because they are request-scoped diagnostics rather than
per-operation payload: they must survive the operation that produced them. Their
packed atomic value records presence separately from the
flags, so even a zero-valued diagnostic is representable.

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

A `RequestContext` is a `plurality::Box<RequestContext>` rented from an instance-owned
pool (§5) and handed to WinHTTP as the opaque handle context. One rule governs its
lifetime: **the driver owns the `Box` until `WinHttpSetOption(CONTEXT_VALUE)`
succeeds; after that WinHTTP owns it and the callback reclaims it on the final
`HANDLE_CLOSING`.**

The status callback is registered once on the session handle at build time (§2,
step S2) with mask
`DISPATCHED_COMPLETIONS | SECURE_FAILURE | HANDLES | CONNECT_TO_SERVER`, and
every request handle inherits it. `DISPATCHED_COMPLETIONS` is narrower than the
native all-completions flag: it omits the proxy-resolution completions, which
the transport can never receive because it resolves proxies through automatic
detection configured on the session rather than through the proxy-resolution
APIs. `CONNECT_TO_SERVER` covers the connection-progress
notifications used by cold-connect telemetry (§12). The only per-request handoff
is installing the context pointer via
`WinHttpSetOption(WINHTTP_OPTION_CONTEXT_VALUE, ptr)`.

**Ordering:** the driver issues this `SetOption` *before* the first async call
(`WinHttpSendRequest`). This guards the window where an async call fails
synchronously: because the context is already installed, closing the handle still
delivers a `HANDLE_CLOSING` that carries the pointer, so the callback reclaims the
`Box` on the normal path instead of the driver needing a second, racy free path. Were
the context installed *after* the send, a synchronous send failure could close the
handle with a null context and strand the `Box`.

The driver applies the other synchronous request options before allocating the
context. A failure there drops the locally owned request and connect handles
directly. Immediately before installing `CONTEXT_VALUE`, the driver allocates the
context and moves the connect handle and session owner into it. If that final option
call fails, the still-owned context returns to the pool, closing the connect handle
and releasing the session owner. This is safe because WinHTTP initializes a handle's
context to null, and the trampoline ignores any callback (including
`HANDLE_CLOSING`) whose context is null. That null check is correctness-critical
rather than defensive: this is precisely the path where the request handle is closed
while its context has already been reclaimed, and dereferencing the echoed value
would be a use-after-free.

After `SetOption` succeeds, dropping the `RequestGuard` owner (the driver or the
`WinHttpBodyReader` it moves into, §6.3) synchronously closes only the request
handle, but does **not** free the context or its retained parents. Closing the
request handle aborts any outstanding operation and makes WinHTTP deliver one final
`HANDLE_CLOSING`, where the trampoline reclaims the `Box`; dropping that box then
closes the connect handle and releases the session `Arc`. Microsoft documents that
closing a parent invalidates child handles and pending asynchronous child requests
cannot be relied on to complete correctly, so retaining both parents until this final
callback is required rather than relying on undocumented native reference counting.

Because the pool's backing memory is reference-counted (like the `Arc`-backed
`events_once` pools), the context stays valid even after the transport or request
future is gone - validity is tied to the callback protocol, not to transport or
request lifetime, the same deferred-free discipline as `oxidizer_io`'s IOCP path.
Reclaiming the `Box` across the FFI boundary uses `plurality::Box::into_raw` /
`from_raw`, so the context pointer both identifies and owns the `RequestContext`
with no side registry.

### 4.4 The request lifecycle is the `RequestDriver`

The "state machine" that issues the calls above is `RequestDriver` in
`request.rs`: the concrete async body that `WinHttpTransport::execute` returns and
polls. It walks the steps of §2, awaiting each step's `events_once` receiver. Before
context installation it owns the request and connect handles plus a session
`Arc<WinHttpSession>` clone. It moves the connect/session parents into
`RequestContext`, then hands that context pointer to WinHTTP through
`WINHTTP_OPTION_CONTEXT_VALUE` and retains a `RequestGuard` containing
the request handle and raw context pointer. Dropping the guard closes the request;
the final callback drops the context and parents as described in §4.3.

### 4.5 Exclusive access without locks

The buffers and sender in `RequestContext` are shared between the driver and callbacks
with no application lock. Safe API sequencing and payload ownership have separate
enforcement mechanisms:

- **The driver arms before issuing the async call.** The mutable `RequestGuard`
  borrow held by the returned future prevents direct guard access. More
  importantly, submission moves the request handle into the future and leaves
  the guard's handle slot empty. Completion destroys the receiver endpoint
  before restoring the handle; cancellation destroys the receiver before
  closing the handle. Even forgetting the future cannot expose a usable guard.
  The driver writes the completion sender and optional buffer, then publishes
  the operation kind with a release store.
- **No exclusive borrow of the `RequestContext` is outstanding across a submission.**
  This is the aliasing rule the whole design rests on, and it is what §1's `Bindings`
  contract and §2.1's reentrancy argument refer to. WinHTTP may complete an operation
  inline and reenter the callback on the submitting thread (§2.1), and that callback
  takes its own shared borrow of the same context; two shared borrows coexist, while a
  shared borrow alongside an exclusive one is undefined behavior. The submitter may
  therefore keep a shared borrow across the call - and does, to arm the slot and pass the
  installed context value - because every state change the submitter and the callback
  share travels through interior mutability: the context's atomic fields and the
  operation slot's `UnsafeCell` payload.
- From the submit call until the `events_once` receiver resolves, the driver
  does not access the operation payload. The context pointer remains shared for
  request diagnostics and callback dispatch throughout the operation (§4.2).
- A **completion** - inline on the submitting thread or later on a worker - validates
  that its status matches the published operation kind and uses compare-exchange
  to claim the payload. Only the winner moves out the sender and buffer and returns
  the slot to `Idle`. We do not expect competing error/success notifications to
  occur at all - WinHTTP serializes callbacks for one request handle - so the claim
  is a correctness backstop, not a race the design plans around. A loser observes a
  failed claim and returns without touching the payload.
- **`HANDLE_CLOSING` must not overlap another callback for the same handle.** The
  atomic tag deliberately does not cover final reclamation: the claim is not atomic
  with the read-out of the payload, and the claimed `events_once` sender points into
  the `EmbeddedEvent` stored inline in the context, so a claimant still touches
  context memory after the tag returns to `Idle`. `HANDLE_CLOSING` frees exactly that
  memory. WinHTTP documents `HANDLE_CLOSING` as the last notification a handle
  receives, and the design relies on that. The slot's destructor
  debug-asserts that it never observes a claim in progress, and its release path
  leaves a claimed payload untouched instead of risking a second destructor run.
- The `events_once` send-then-receive is the release/acquire edge that transfers
  buffer ownership back to the driver. Only after the receiver resolves does the
  driver read the returned buffer.

The context pointer itself remains shared: after installation, neither the request
task nor a non-final callback creates an exclusive reference to `RequestContext`.
Mutation is restricted to atomic fields and the operation slot's `UnsafeCell`
payload under its atomic ownership tag. `HANDLE_CLOSING`, documented as the final
notification with no later callbacks, is the only path that reconstructs and drops
the owning box.

**The one field that is not covered by the temporal handoff** is the packed
`RequestContext::secure_failure` atomic. WinHTTP serializes callbacks for a single
request handle, and `SECURE_FAILURE` in practice precedes the `REQUEST_ERROR` raised
by the same send: both are emitted from the send-completion path, the secure-failure
flags being already-cached handshake results rather than a schannel worker's late
report. The field is nevertheless atomic rather than a `Cell` because successive
callbacks for one handle run on *different* WinHTTP worker threads. The
happens-before edge that orders them lives inside WinHTTP's own lock, which is
invisible to the Rust memory model, so a `Cell` would be both unsound and `!Sync`.
A `SECURE_FAILURE` status publishes its certificate-error bitmask into that atomic
with a `Release` store and touches nothing else. `REQUEST_ERROR` classifies the failure
from its WinHTTP error code - never from callback order - and reads the atomic through
the `secure_failure_flags()`
accessor, whose `Acquire` load attaches whatever diagnostic flags have arrived to
the resulting `WinHttpError::secure_failure_flags`. A later `SECURE_FAILURE` can still
update request-scoped diagnostics without touching an operation sender or buffer. Every
branch remains non-blocking
(§2.1).

### 4.6 The outer connect-timeout race

The total connect deadline design.md §6.2 requires is the transport's
self-scheduled request-phase timer. Response timeout is already wrapped around the
pipeline by `fetch`, body idle timeout is applied by `HttpBodyBuilder`, and only an
explicit DNS-only timeout uses a native WinHTTP timer (§10.4). The
`RequestDriver` races the connect/send phase against a single
`tick::Clock::delay(connect_timeout)`, using the clock already threaded in from
`CustomContext` (no new dependency). Whichever finishes first wins. If the timer
fires, the operation future snapshots cold-connect attribution while it still
owns the live request, then drops its receiver and closes the handle; this
cancels the in-flight connect (§4.3) without dereferencing the context after an
inline `HANDLE_CLOSING`. The driver then returns `HttpError::timeout`. If the
connect completes first, the timer future is dropped. `tick::Clock` is the sole
source of time on this path; the transport never calls `std::thread::sleep` or
`tokio::time`.

## 5. Context and buffer pooling

Each request issues several async steps, so its pinned `RequestContext` embeds one
reusable `events_once::EmbeddedEvent<CompletionResult>`. This avoids per-step
allocation without making callback-side endpoint reclamation acquire the internal
mutex used by `events_once::EventPool`. The receiver's mutable borrow of
`RequestGuard` and ownership of its request handle prevent another operation
from reinitializing the embedded storage before the current receiver endpoint
has been destroyed.

- **`plurality::Pool<RequestContext>` behind a `Mutex`.** A request rents one
  `plurality::Box<RequestContext>` at start and holds it for its whole lifetime
  (reclaimed when the callback drops it on `HANDLE_CLOSING`, §4.3). `plurality::Pool`
  is `Send + !Sync`, so it is the one field we wrap in a `std::sync::Mutex`. That lock
  guards allocation alone: `ContextInstallation::new` takes it to call `Pool::alloc_box`
  and nothing else takes it - never across an `.await`, never by the body reader (which
  reuses the already-rented context), and never on a callback path. Reclamation needs no
  lock because installation detached the rented box with `plurality::Box::into_raw`;
  `HANDLE_CLOSING` reconstructs it with `plurality::Box::from_raw` and drops it, and that
  `Drop` returns the slot to the pool on its own.

Read buffers come from the separate shared memory pool. `WinHttpDeps` retains a clone
of its mandatory `bytesbuf::mem::GlobalPool` in the transport extras while also
supplying that pool to `fetch::custom::CustomDeps` for the response
`HttpBodyBuilder`. Each materialized
transport receives the retained clone through `CustomContext::extras`; the body reader
clones it and rents buffers with no lock.

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
(pull-based `poll_frame`). The WinHTTP write side is a `bytesbuf_io::Write`, implemented in
`body/write.rs`; `body/mod.rs` contains only module declarations and re-exports:

```rust,ignore
impl bytesbuf_io::Write for WinHttpBodyWriter {
    type Error = HttpError;
    async fn write(&mut self, data: BytesView) -> Result<(), HttpError> {
        // Stores `data` and the submitted span length in
        // OperationBuffer::Write, issues WinHttpWriteData, and awaits
        // WRITE_COMPLETE via an events_once step. The BytesView remains
        // retained and unchanged until the callback releases it (§4).
    }
}
```

The sending strategy is chosen by the request driver's upload stage. In **either** case
`WinHttpSendRequest` is called with a `NULL`/zero `lpOptional` buffer; the request
body is never passed inline to `WinHttpSendRequest` and is always streamed with
`WinHttpWriteData`. This sidesteps the `lpOptional` lifetime rule (an inline
optional buffer would have to stay valid through `WinHttpReceiveResponse` or
cancellation completion) and keeps a single body-writing path:

- **Known length** (buffered body / `Content-Length`): the total length passed to
  `WinHttpSendRequest` (`dwTotalLength`) is a `DWORD`. When the known length fits
  in `u32`, pass it directly and stream frames through `WinHttpBodyWriter`. When it
  exceeds `u32::MAX`, `dwTotalLength` cannot represent it, so instead pass
  `WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH` and set an explicit 64-bit `Content-Length`
  request header (WinHTTP honors a caller-supplied `Content-Length` and does not
  fall back to chunked when it is present), then stream the body identically. Either
  way there is one write path.
- **Unknown length** (streaming body): open the request with
  `WINHTTP_FLAG_AUTOMATIC_CHUNKING`, then call `WinHttpSendRequest` with
  `WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH` and a `NULL` optional buffer. Each
  `poll_frame` data chunk is pulled and written sequentially. After the body reaches
  end-of-stream, a final `WinHttpWriteData` with a `NULL` buffer and zero length is
  completed before proceeding to `WinHttpReceiveResponse`; this tells WinHTTP to finish
  its protocol-appropriate HTTP/1.1, HTTP/2, or HTTP/3 framing. The transport never has
  send and receive operations outstanding together.

`HttpBody` may also yield a trailer frame. WinHTTP has no API for submitting request
trailers after the body, so the writer returns an `HttpError` when it encounters one; it
never silently discards the frame or writes trailer bytes as body data.

Each individual `WinHttpWriteData` length is also a `DWORD`, so a single
`BytesView` larger than `u32::MAX` is written across successive `WRITE_COMPLETE` steps,
independently of how the total length is declared above. `BytesView` may be segmented:
every call receives exactly one contiguous span, further bounded by `u32::MAX`; the
writer advances through all spans without pairing an aggregate length with only the first
span's pointer.

No independent `Content-Length` mismatch state machine runs during the upload itself;
framing metadata is instead reconciled once, up front, by `RequestBodyFraming::new`
(`body/write.rs`). `RequestDriver::new` calls it during request translation - before the
connect and request handles are opened and before any network I/O - passing the caller's
header map and the length the body reports for itself. It both selects the strategy above
and normalizes the headers that `TranslatedRequest` then serializes, so exactly one
framing directive reaches the wire. Each rejection below is generated locally as an
`HttpError` labeled `invalid_request` with `RecoveryInfo::never()`; none of them
originates in WinHTTP:

- A caller-supplied `Transfer-Encoding` fails with `UnsupportedTransferEncodingError` in
  every framing mode. WinHTTP performs all request framing itself, so an already-encoded
  body would be encoded a second time, and forwarding the header would put a second
  framing directive next to WinHTTP's own - a combination RFC 9112 resolves in favor of
  `Transfer-Encoding`, and the classic request-smuggling primitive.
- When the body reports no length of its own, `Content-Length` supplies it, and the value
  is taken on trust: it becomes the length the transport frames against, with nothing to
  check it against. A value that is empty, non-decimal, or overflows `u64`, and duplicate
  values that disagree with each other, leave the declared length ambiguous and fail with
  `InconsistentContentLengthError`.
- For **every** known length, not only lengths above `u32::MAX`, each present
  `Content-Length` value must equal the length the framing uses. A disagreement fails with
  `MismatchedContentLengthError` below the `DWORD` boundary and
  `LargeContentLengthMismatchError` above it, the latter naming the exact 64-bit length
  required. The comparison catches an untruthful header only where the body reported its
  own length; where the length was adopted from the header it compares the value with
  itself and normalizes. Values that survive are collapsed into one canonical decimal
  header, so duplicates and non-canonical spellings such as `007` never reach the wire.
- When the caller supplied no `Content-Length` and the length fits a `DWORD`, none is
  inserted: `WinHttpSendRequest` emits the header from `dwTotalLength`. Above `u32::MAX`
  the header is the only framing source, so it is inserted.

Declared-length or framing failures that WinHTTP detects later - after this
reconciliation, on the wire - are propagated through the ordinary request error path.

### 6.2 Incoming response body -> an `http_body::Body`

The read side is a reader over WinHTTP exposing a single inherent read operation:

```rust,ignore
impl WinHttpBodyReader {
    async fn read_into(&mut self, into: BytesBuf) -> Result<(usize, BytesBuf), HttpError> {
        // 1. WinHttpQueryDataAvailable -> DATA_AVAILABLE(n)
        // 2. choose a desired size from n, or the speculative default when
        //    n == 0, then reserve that much capacity (§6.2.1)
        // 3. retain `into`, the exposed region address, and its submitted
        //    capacity in OperationBuffer::Read
        // 4. WinHttpReadData reads min(desired, first contiguous unfilled
        //    region length, u32::MAX) bytes
        // 5. READ_COMPLETE validates the returned pointer/length against that
        //    address and capacity before returning (len, buffer), which
        //    transfers BytesBuf ownership back here; len == 0 means EOF
    }
}
```

EOF is taken from a **zero-length `READ_COMPLETE`**, not from
`WinHttpQueryDataAvailable` returning 0. Both usually coincide, but WinHTTP's
documented completion signal is the zero-length read, and reading directly avoids
depending on `QueryDataAvailable`'s value for correctness. `QueryDataAvailable` is
used only to size ordinary reads. The authoritative "body finished" decision is a
`READ_COMPLETE` with `len == 0`.

After the zero-length EOF read, the reader calls `query_raw_trailers`, implemented as
`WinHttpQueryHeaders(WINHTTP_QUERY_RAW_HEADERS_CRLF |
WINHTTP_QUERY_FLAG_TRAILERS | WINHTTP_QUERY_FLAG_WIRE_ENCODING)`. A missing trailer
block becomes `None`; returned trailer bytes are parsed into a `HeaderMap`. The final
response adapter is a custom
`http_body::Body`, passed through `HttpBodyBuilder::body`, so it yields data frames and
one final trailer frame instead of erasing trailers through a data-only stream conversion.
The adapter owns the reader between reads and lends it to the in-flight read, which keeps
that boxed future free of borrows:

```rust,ignore
let body = WinHttpResponseBody::new(WinHttpBodyReader::new(/* .. */));
let body = builder.body(body, &body_options);
```

The resulting `HttpBody` is pull-based:
WinHTTP reads are issued lazily as the consumer polls, so backpressure is natural and
there is no unbounded buffering. The request's body idle timeout
(`http_extensions::BodyTimeout`) is copied into `body_options`.
`HttpBodyBuilder::body` merges it with the client's response-body defaults and applies
the Rust-side idle-timeout wrapper (§10.4). Native WinHTTP receive timers remain
unlimited.

The `READ_COMPLETE` buffer WinHTTP fills is a slice reserved inside a pooled
`BytesBuf`; it stays pinned until the callback fires (§4), then the filled prefix
is yielded as a zero-copy `BytesView`.

#### 6.2.1 Read buffer sizing

Three constraints shape how much memory a read reserves and how much of it a single
`WinHttpReadData` call receives.

**WinHTTP writes into one contiguous region.** A `BytesBuf` reservation may be
segmented across several pooled blocks, so each call is given
`BytesBuf::first_unfilled_slice()` and a length bounded by that region and by
`u32::MAX`. The reader never pairs a logical buffer length with a pointer into only
its first block. Nothing here assumes a particular block size: the bound is read from
the buffer at runtime, so `GlobalPool` remains free to change its size classes.

**Large reads are preferred when no better figure is known.** `PREFERRED_READ_SIZE`
is 256 KiB, matching the default I/O size used across related projects. It caps every
reservation and supplies the size outright when `QueryDataAvailable` reports zero and
the read is therefore speculative. Larger submissions amortize the cost of the WinHTTP
call and its worker-thread handoff over more bytes.

**Leftover capacity becomes a local reserve.** A reservation that exceeds the first
contiguous region leaves the remaining blocks in the `BytesBuf`. `BytesBuf::reserve`
is a no-op while the buffer already holds enough capacity, so subsequent reads take
their region from that reserve and reach `GlobalPool` again only once it is spent.
Blocks therefore are not returned to the pool and re-rented between consecutive reads
of one body.

**Retaining the buffer across frames is a security property, not only an
efficiency one.** A pooled block stays rented for as long as any `BytesView` cut
from it is alive, so handing each emitted frame the whole buffer it was read into
would let a peer that trickles bytes rent one block per byte delivered - retained
memory bounded by the attacker's frame count rather than by the payload. The adapter
instead detaches only the filled prefix and keeps the buffer, so consecutive small
reads refill capacity the outstanding frames already pin. Combined with sizing
ordinary reservations from the queried availability rather than from
`PREFERRED_READ_SIZE`, retained memory tracks bytes actually delivered.

### 6.3 End-to-end request lifecycle (`RequestDriver` in `request.rs`)

`WinHttpTransport::execute(req)` returns a `Send` future implemented by
`RequestDriver`, which runs (each `->async` awaits an `events_once` completion;
§3.3):

```text
translate req (method/uri/headers -> UTF-16)
  -> open connect handle (inline WinHttpConnect; non-blocking, no cache, §9.1)
  -> WinHttpOpenRequest + set options (protocol, decompression, redirect, cookies/auth off, security, timeouts)
  -> set RequestContext pointer as WINHTTP_OPTION_CONTEXT_VALUE
  -> WinHttpSendRequest ->async SENDREQUEST_COMPLETE
  -> poll HttpBody frame -> WinHttpWriteData ->async WRITE_COMPLETE  [repeat through end-of-stream]
  -> unknown length only: zero-length WinHttpWriteData ->async WRITE_COMPLETE
  -> WinHttpReceiveResponse ->async HEADERS_AVAILABLE
  -> WinHttpQueryHeaders/Option (status, negotiated version, header block)  [sync]
  -> move RequestGuard into WinHttpBodyReader
  -> build HttpResponse { parts, lazy body } through HttpResponseBuilder::body
  -> return Ok(response)
  -> on body poll: QueryDataAvailable ->async DATA_AVAILABLE
     -> ReadData ->async READ_COMPLETE  [repeat until zero-length completion]
  -> query and emit response trailers, if present
  -> close request; HANDLE_CLOSING later reclaims context and parents
```

Header translation is mechanical. Request header names and values are written
directly from their validated `http` byte representations into one NUL-terminated
UTF-16 CRLF blob, preserving repeated fields and opaque header-value bytes without an
intermediate UTF-8 string. Method, authority host, and path/query are materialized as
separate UTF-16 inputs from the `http::Request` parts. An omitted path becomes `/`;
a query-only path is prefixed with `/` before it is passed to WinHTTP.

After `HEADERS_AVAILABLE`, the numeric status is queried with
`WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER`. The raw response header block
is queried with `WINHTTP_QUERY_RAW_HEADERS_CRLF |
WINHTTP_QUERY_FLAG_WIRE_ENCODING`, so WinHTTP writes wire-encoded bytes directly rather
than first converting them to UTF-16. The two-call query treats both the required
capacity and returned length as byte counts: it allocates exactly the reported byte
capacity, then truncates to the returned byte length. Parsing operates on `&[u8]` line
by line. Response headers require and skip the status line; response trailers use the
same field parser without a status line. Names must be ASCII, optional whitespace is
trimmed, opaque `obs-text` value bytes are preserved, and repeated fields are appended
to the `http::HeaderMap` rather than overwritten. Invalid header names and values are
rejected. `WINHTTP_QUERY_FLAG_WIRE_ENCODING` and the trailer query flag establish the
platform minimum documented in design.md section 1.3; no runtime compatibility path
is attempted. Only the raw header and trailer blocks are queried as wire-encoded bytes:
the numeric status query uses a `DWORD` buffer and the legacy `WINHTTP_QUERY_VERSION`
string query a UTF-16 buffer.

The response lifecycle constructs a lazy `WinHttpResponseBody` through
`HttpBodyBuilder::body`, attaches no `ConnectionInfo`, and moves `RequestGuard` into
`WinHttpBodyReader` after all response metadata has been queried. No response-body call
is made before the caller polls the body. EOF, a body error, timeout, or body drop closes
the request handle. The context retains the connect handle, session owner, and any
active operation buffer until the resulting `HANDLE_CLOSING` callback reclaims it.

The upload lifecycle polls every outgoing body frame lazily after
`SENDREQUEST_COMPLETE`. Empty data frames are inert. Each nonempty data frame is
written one contiguous `BytesView` span at a time, further split at `u32::MAX`, and the
next frame is not polled until every write for the current frame completes. Body-stream
errors propagate directly, and a trailer frame fails with `invalid_request` because
WinHTTP cannot submit request trailers. Only after end-of-stream does the driver issue
`WinHttpReceiveResponse`, so request upload and response reception are never concurrent.

Before driver execution, the transport captures `HttpBody::try_clone()` when the
outgoing body is replayable and tracks whether `poll_frame` has been attempted. An
error before the first poll retains the unchanged request. After the first poll, the
request is attached only when the captured body clone can replace the consumed body;
non-cloneable streaming requests are omitted from the error. Replacement changes only
the body, leaving the request method, URI, version, headers, and extensions untouched.
The transport first removes any attachment supplied by a propagated body error, so only
this eligibility policy can expose a request to the retry layer. This makes an attached
request eligible for replay rather than exposing a consumed suffix or an unrelated
request to the retry layer.

The lazy response-body implementation extends this ownership path by moving the guard
into `WinHttpBodyReader`; that reader then becomes the same single close authority on
EOF, read error, or body drop.

## 7. Testing

The bindings facade (§1) makes the transport testable without the OS. Unit tests
drive the `RequestDriver` against `MockBindings` with synthesized callbacks;
integration tests exercise the real OS against a localhost server (§1.2).
Production code and mock unit tests do not depend on Tokio; the localhost TLS,
HTTP/2, and HTTP/3 fixtures use Tokio as a dev-only dependency.

**Miri.** The FFI path cannot run under Miri, so the cancellation/leak invariants
(§4) are asserted on the mock path.
The mock path exercises the same allocate/leak/reclaim code as production so that
"free exactly once on `HANDLE_CLOSING`" is verified where Miri is available;
real integration tests run outside Miri and are `#[cfg_attr(miri, ignore)]`. Miri
coverage is not a separate concern from the test strategy but a property of it:
the mock-bindings unit tests are written so the ownership-critical paths run and
are checked under Miri.

### 7.1 Unit tests (mock bindings, no network)

Each module's unit tests build the code under test over `MockBindings` with a local
harness: `request.rs` scripts a whole request through `lifecycle_bindings`/`run_lifecycle`
plus a `LifecycleRecord` of the calls the driver made, and `body/read.rs` scripts
availability and read steps behind its own reader harness. Callbacks are fired by calling
`dispatch_completion` with the installed context pointer, which is exactly what the real
trampoline does, so completions can be interleaved deterministically - inline on the
submitting thread, from another thread, or after cancellation - and out-of-order or late
notifications can be injected.

### 7.2 How each key factor is tested

Most factors are checked by asserting the calls and values the driver hands the mock.
The factors with nontrivial ordering - reentrant completion, cancellation / FFI
ownership, and body streaming - need scripted sequences and are described as prose
after the table.

| Factor | Key assertions | Notable adverse / edge case |
|--------|----------------|-----------------------------|
| Threading (§3) | completions fired from a foreign OS thread reach the awaiting future; `static_assertions` for `execute`'s future `Send`, handles `Send`+`!Sync`, handler `Send + Sync`, and instance-owned pools | all setup calls run inline on the caller's thread |
| Error handling (design.md §7) | table-driven Win32/`WINHTTP_*` code -> `ErrorLabel` + `RecoveryInfo`, including an unrecognized code mapping to `request_winhttp` with unknown recovery; `GetLastError` mapping on a failing synchronous call | a 4xx/5xx response is `Ok`, not `Err` |
| Protocol negotiation (design.md §3) | protocol-flag bitmask + `HTTP_PROTOCOL_REQUIRED` per `supported_http_versions` (empty -> `fetch` default; h2/h3-only -> required); response `Version` from the queried negotiated protocol | unmappable requested version (`HTTP/1.0`, `HTTP/0.9`) rejected as `invalid_request` |
| TLS (design.md §4) | `WINHTTP_FLAG_SECURE` iff `https`; security-flags bitmask per `accept_invalid_*`, each flag setting only its own `SECURITY_FLAGS` bit (they are independent, not coupled); WinHTTP secure error code -> `tls` label, with deterministic validation failures non-retryable and revocation-server unavailability retryable; `SECURE_FAILURE` flags are optional diagnostics | mTLS out of scope (design.md §4.1) - nothing to assert |
| Compression / redirects / statelessness (design.md §5) | `DECOMPRESSION`, `REDIRECT_POLICY_NEVER`, `DISABLE_COOKIES`, `DISABLE_AUTHENTICATION` set; an already-decoded body streams untouched; a 3xx is surfaced verbatim | brotli/zstd response passes through still-encoded |
| Connection management (design.md §2) | connect handle opened per request and retained until the request's final close callback; finite `max_connections` causes no max-conns option call; `ConnectionKeepAlive` maps to `HTTP2/3_KEEPALIVE`, with the 5000 ms floor applied to HTTP/2 (§10.3); `connection_idle_timeout` maps to `CONNECTION_IDLE_TIMEOUT` on the session, with the same 5000 ms floor and `Unlimited` encoded as the largest `DWORD`; `DISABLE_GLOBAL_POOLING` on the session | generic lifetime settings are accepted and ignored without diagnostics |
| Timeouts (design.md §6) | native timers initialize to unlimited and stay there; frozen-clock connect deadline (design.md §6.2); `ResponseTimeout` remains owned by `fetch`; `BodyTimeout` is passed to `HttpBodyBuilder` | a connect completing first drops its timer unfired; request body options override client body defaults through the existing merge rules |

- **Inline / reentrant completion.** `MockBindings` is configured so an async call
  (e.g. `read_data`) fires its completion *synchronously, inline, on the submitting
  thread* before returning - the reentrant case `ASSURED_NON_BLOCKING_CALLBACKS`
  permits (§2.1). The tests assert the driver still observes the result correctly (the
  `events_once` send lands before the receiver is awaited), and running them under Miri
  checks the aliasing rule of §4.5 on exactly that path, where a submitter borrow and a
  callback borrow of one `RequestContext` are live together. Both
  `SECURE_FAILURE`/`REQUEST_ERROR` orders run, and error-code classification never
  depends on which notification arrived first.
- **Cancellation and FFI ownership.** The centerpiece. (1) The response
  body is dropped while a `READ_COMPLETE` is outstanding; `close_handle` is called and the pooled
  `RequestContext` is not returned to the pool until the harness fires the
  synthetic `HANDLE_CLOSING`, after which it is returned exactly once (the mock
  records alloc/free; this runs under Miri where available). (2) Cancellation with an outstanding
  write. (3) `ERROR_WINHTTP_OPERATION_CANCELLED` delivered after close is swallowed
  (no waiter) without UB. (4) The pooled `Box` and rented `events_once` events return
  to their instance-owned pools. (5) **Setup-failure leak-freedom:** the context
  set-option (`WINHTTP_OPTION_CONTEXT_VALUE`) fails; the `Box` returns to the pool inline,
  no leak. (6) **Null-context guard:** a `HANDLE_CLOSING` for a handle whose context
  was never installed (early-failed request, or a connect/session handle) is ignored
  and reconstructs no `Box`. (7) The session-level status callback is registered once
  with the full notification mask (§4.3). (8) **Session lifetime:** the last
  transport instance / session `Arc` is dropped while a response body is mid-read; the
  context's retained session `Arc` keeps the session wrapper alive so the in-flight
  read completes, and request-guard drop closes only the request. The synthetic
  `HANDLE_CLOSING` then reclaims the context, closes the connect handle, and releases
  the session owner exactly once (this runs under Miri where available).
- **Body streaming.** The response body reader is driven with a scripted
  `DATA_AVAILABLE`/`READ_COMPLETE` sequence: EOF is taken from a
  zero-length `READ_COMPLETE` (not from `QueryDataAvailable`), `ReadComplete`
  returns the same pooled `BytesBuf` (ownership round-trip) with the correct appended
  length, no read is issued until the consumer polls `poll_frame`
  (backpressure), and a mid-stream error propagates. For the writer, scripted `WRITE_COMPLETE`s
  across frames establish chunk-by-chunk `WinHttpWriteData` with correct
  pointers/lengths and buffer pinning, a `BytesView` larger than `u32::MAX` split
  into `u32`-sized writes, and a known body length above `u32::MAX` using
  `WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH` plus an explicit `Content-Length` (§6.1).
  Framing reconciliation is asserted directly on `RequestBodyFraming`: agreeing
  duplicates collapse to one canonical value, a disagreeing, malformed or overflowing
  `Content-Length` fails, a known length that fits a `DWORD` and carries no header keeps
  none, and a caller-supplied `Transfer-Encoding` is rejected in every framing mode.
  Request-trailer rejection and response-trailer parsing/emission are exercised through the
  public `HttpBody` surface.

### 7.3 Integration tests (real WinHTTP, localhost)

Gated behind `#[cfg(windows)]` and `#[cfg_attr(miri, ignore)]`, against the localhost
fixtures in `fetch_winhttp_testing`. `TestServer` (`server.rs`) serves plaintext or TLS traffic
over an ephemeral TCP port on the loopback address using `hyper` plus `hyper-util`'s
protocol-detecting connection builder, so one instance answers both HTTP/1.1 and HTTP/2;
its TLS mode wraps that with `tokio-rustls` and an `rcgen` self-signed certificate whose
names each test chooses. `Http3Server` (`http3_server.rs`) serves HTTP/3 over an ephemeral
UDP port with `quinn` plus `h3` and its own self-signed certificate advertising the `h3`
ALPN protocol. Both fixtures run a Tokio runtime on a dedicated thread, script their
responses up front, and record every received request together with a connection counter,
which is what the isolation and reuse assertions below read. §1.2 lists which binary
owns which of the behaviors below. They validate the real OS
path end to end:

- GET/POST with small and large bodies; response body correctness and size.
- Unknown-length streaming uploads over HTTP/1.1, HTTP/2, and HTTP/3, followed by
  `WinHttpReceiveResponse` only after the final write; streaming downloads. Mock
  tests assert incremental frame submission and completion ordering, while localhost
  tests assert the final bytes, negotiated protocol, and HTTP/1.1 chunked framing.
- Request trailer frames fail explicitly. Response trailers are preserved for HTTP/2
  and HTTP/3; WinHTTP does not expose them for HTTP/1.1.
- Real gzip/deflate responses are transparently decoded.
- Redirects are never followed (`REDIRECT_POLICY_NEVER`, design.md §5 and
  implementation.md §10.3): a request to a
  localhost endpoint returning a 302 whose `Location` points at a sentinel endpoint
  confirms the 3xx status and `Location` header are surfaced unchanged and that the
  sentinel endpoint is never hit (server-side hit counter stays zero). This proves the
  option overrides WinHTTP's default follow-redirects behavior and is set at the right
  handle scope.
- Cookies are never stored or replayed (`DISABLE_COOKIES`, design.md §5 and
  implementation.md §10.3): a first response
  sets `Set-Cookie`; a second request to the same authority asserts no `Cookie` header is
  attached (verified server-side). This proves WinHTTP's default cookie jar is disabled at
  the correct handle scope.
- `https` against localhost TLS fixtures exercises `accept_invalid_certs` and
  `accept_invalid_hostnames` as independent relaxations without installing a test CA.
  A self-signed certificate with a valid localhost name proves certificate relaxation;
  a self-signed certificate with a hostname mismatch proves that only enabling both flags
  accepts both faults. Exact security-flag unit tests cover every individual bit.
  (Client-certificate/mTLS is out of scope for v1, design.md §4.1.)
- Pool isolation across clients: two `HttpClient`s built independently - including two
  builds of a *cloned* `builder_winhttp` builder - issue requests to the same authority;
  server-side connection counting shows that they establish *separate* connections
  and never reuse each other's, proving the per-built-client session/pool boundary
  (§3.2, design.md §2).
- Pool isolation across slots within one client: a single `HttpClient` built with
  `multiple_pools(2)` routes requests through both pool slots to the same authority;
  server-side connection counting shows that a connection opened for one slot is
  never reused by the other, proving each slot lands in its own session/pool even though
  the transport ignores the `PoolIndex` *value* (§8). A mock-bindings unit test pairs with
  it by asserting that `WinHttpOpen` runs once per slot (§3.2).
- HTTP/1.1 vs HTTP/2 negotiation against `TestServer`, whose connection builder accepts
  both; the reported response `Version` names the negotiated protocol.
- HTTP/3: `TestServer` speaks no HTTP/3, so h3 is tested against `Http3Server` using its
  self-signed certificate and `accept_invalid_certs`. The negotiated `Version` is
  HTTP/3, and the "h3 required but QUIC unreachable" path yields the
  expected failure (`0x2EFE`/`0x2EFD`).
- Connection reuse: two sequential requests to the same authority reuse the
  connection (observable via server-side connection counting).
- Timeout configuration is validated only structurally (unit, §7.4). Integration
  clients are built on a frozen `tick::Clock` that no test advances, and the native
  WinHTTP timers are left unlimited, so no `fetch`-layer or transport-scheduled
  deadline can fire during a run and a timing race can never masquerade as a failure.
  No integration test asserts a timeout *firing* against a slow/black-hole endpoint,
  because that would depend on real wall-clock timing and be flaky.
- Real cancellation: an in-flight download future is dropped and teardown is clean
  (no panic, no leak), the integration counterpart to the unit cancellation tests.
- Lifecycle soak: a large batch mixes normal completion, pending-body cancellation,
  and response drops, then performs another request to prove the client remains usable.
  Exact context allocation/reclaim and `HANDLE_CLOSING` ownership invariants are
  asserted by mock unit tests under Miri.

The full `fetch` pipeline (retry/breaker/telemetry) is validated by building an
`HttpClient` via `HttpClient::builder_winhttp(...)` and asserting a real request round-trips,
mirroring `fetch`'s existing `requests` integration test structure.

### 7.4 Timeout testing

Timeout *configuration* is asserted in unit tests: the mock bindings record the
`WinHttpSetTimeouts` arguments, so tests assert that every native timer is programmed
unlimited. `ResponseTimeout` coverage remains in `fetch::HttpClient`, while body tests assert
that the request's `BodyTimeout` reaches `HttpBodyBuilder` and merges with client-level
body options.

The one transport-scheduled timer - the outer connect timeout (design.md §6.2) - *is*
driven by `tick::Clock`, so it is unit-testable deterministically: with a frozen
clock and mock bindings that never complete the connect, a test advances the clock
past `connect_timeout` and asserts the driver closes the handle and yields
`HttpError::timeout`, and conversely that a connect completing before the deadline
drops the timer without firing.

Timeout *firing against the real OS* cannot be made deterministic: native WinHTTP timers
use the real OS clock, which tests cannot freeze or fast-forward. Real-time integration
tests are therefore unacceptable. Integration tests leave native timers unlimited and
build their clients on a frozen `tick::Clock` that no test advances, so every
`fetch`-layer and transport-scheduled deadline is structurally unable to fire. The
connect deadline and response/body timeout behavior are covered with controlled clocks.

## 8. Client construction

`HttpClientWinHttpExt::builder_winhttp` (design.md §1.1) does not reimplement any
pipeline wiring; it delegates to `fetch`'s custom-transport entry point, calling
`fetch::custom::create_builder("winhttp", "winhttp", factory, Isolation::Isolated, deps)`.
There is no `new_winhttp`: the timer-capable `Clock`, `GlobalPool`, and `Sink` are
mandatory environment dependencies and have no runtime-neutral defaults. They are passed
to `WinHttpDeps::builder(clock, global_pool, sink)`. TLS and WinHTTP-specific user
configuration default when omitted. The `create_builder` signature this targets is:

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
`Meter`, and the caller's `Extras`. `fetch_winhttp` ignores `PoolIndex` (per-processor
placement comes from `Isolation::Isolated`, §3.2) and ignores `CustomContext::tls` (it
takes its own `WinHttpTlsConfig` instead; see design.md §1.2). This generic TLS
configuration is ignored without a runtime warning; the limitation is part of the
documented transport contract. Ignoring the `PoolIndex`
*value* does not collapse `fetch`'s `multiple_pools`: `fetch` invokes the factory once per
pool slot (`0..pool_count` in `client_builder.rs`), so each slot opens its own WinHTTP
session (§3.2), and because pooling is per-session (`DISABLE_GLOBAL_POOLING`, §9.3) those
sessions already hold distinct pools. Distinct `PoolIndex` slots therefore land in distinct
sessions/pools structurally, without the transport keying anything on the index. The real
v1 resource profile is one session/pool per (processor × pool slot). Whether connection-pool
ownership belongs on `fetch` at all or entirely on the transport is unresolved and may
retire the `PoolIndex` surface in its current shape (../../fetch/docs/stabilization.md,
connection-management item).

`builder_winhttp` does **not** open the session; it just calls `create_builder` with the
factory. Each materialized (processor × pool-slot) transport instance opens its own session
inside the factory when
`fetch` materializes it (§3.2), so the session is scoped to the built client and never
captured in the clone-shared builder closure. The session is deliberately not a
`WinHttpDeps` field either - `WinHttpDeps` stays plain, relocatable configuration. The
clock comes from `CustomContext`. Because `CustomContext` exposes only the derived
`HttpBodyBuilder`, not the underlying `GlobalPool`, `WinHttpDeps` also retains a pool
clone in `Extras` for WinHTTP read buffers. The `observed::Sink` rides in the same extras
and relocates per processor with the rest of the config; the transport emits its telemetry
through it (§12). There is no
`anyspawn::Spawner`: no WinHTTP call the transport makes can block (§2.1).

Session creation uses automatic proxy discovery and applies every required session
option without an old-Windows capability-probing or degradation path. Because the
custom-transport factory is infallible, a session that cannot be opened or configured
produces a permanently failed handler. Every request to that handler returns a fresh
initialization `HttpError` without opening request/connect handles or issuing network
I/O, which is the contract design.md §7 states.

Error construction follows one shape for native failures: a Win32/`WINHTTP_*` code becomes
`HttpError::other(WinHttpError { code, operation, secure_failure_flags }, recovery,
label)`, where `error.rs` derives the label and `RecoveryInfo` from the code alone.
`WinHttpError` is `pub(crate)` and is not on the `allowed_external_types` allowlist, so a
caller can neither name nor downcast to it; the code reaches callers only through the
source error's `Display` output, exactly as design.md §7 promises. Other families do not
wrap a `WinHttpError` directly: the initialization failure above wraps a
`SessionInitializationFailure` that names the failed setup step and keeps its
`WinHttpError` as its own source; locally rejected requests (§6.1, §10.1) wrap the
translation or framing error that describes what the caller must change
(`error::invalid_request`); and response metadata a successful native call returned but
the transport cannot use, together with callback sequences that contradict the
asynchronous model, wrap the parse or protocol description alone
(`error::invalid_response`, `error::callback_protocol_error`). `error::query_error` is the
router between the first shape and the third: a `QueryError::WinHttp` keeps its code, a
`QueryError::Conversion` becomes an invalid response.

Further outcomes construct no transport error at all. The transport-scheduled connect
deadline (§4.6) returns `HttpError::timeout`, which describes the configured deadline
rather than an operating-system condition, and an error raised by the caller's own
request body stream propagates unchanged so the caller sees the error it produced.

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
`fetch_winhttp` opens one per request via `WinHttpConnect` and retains it until
the request handle's final `HANDLE_CLOSING` callback. There is **no**
connect-handle cache. Caching would add shared
mutable state (a per-authority map, which would need
synchronization) to save a call that does no I/O, and it would buy no connection
reuse: reuse is keyed by authority in the session's pool and is independent of
which connect handle a request used. Dropping the cache keeps the transport's
shared state limited to the read-only session (§3.2).

### 9.2 HTTP/1.1 serialization and concurrency

For HTTP/1.1 there is no multiplexing: concurrent requests to the same authority
are serviced by separate pooled connections under WinHTTP's own limits. The transport
does not set `WINHTTP_OPTION_MAX_CONNS_PER_SERVER`: `fetch`'s finite
`max_connections` value limits idle retained connections, while the WinHTTP option
limits all physical connections and could throttle active requests. Finite values are
therefore ignored without a runtime warning, as documented in design.md §2.1.
HTTP/2 and HTTP/3 multiplex many requests over a single connection, also handled by
WinHTTP.

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
own pool (one pool per (processor × pool slot) under the per-session model, §3.2)
while different clients stay isolated. Reuse within a client is unaffected.

For draining, WinHTTP exposes only coarse controls:

- `PurgeKeepAlives` on the session retires idle pooled connections session-wide.
  There is no API to drain one specific connection.
- Per request, `WINHTTP_OPTION_DISABLE_FEATURE | WINHTTP_DISABLE_KEEP_ALIVE`
  prevents that request from keeping or reusing a pooled connection.

Individual pooled connections are largely opaque to us: WinHTTP exposes no
per-connection handle, age, or close, so a graceful drain of a single connection
is not expressible with the controls this transport uses today. It does expose
per-connection *identity*, which §13 records as an unexploited opportunity, but
identity alone does not make a single connection closable. The v1 transport keeps
exactly one session for its whole lifetime and does not recycle it.
Connection-lifetime handling under those controls is covered in design.md §2.2.

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

The version set from design.md §3 maps to WinHTTP request options as follows.

v1 assumes the modern WinHTTP option set required by this design. It performs no
`WINHTTP_OPTION_FEATURE_SUPPORTED` probes and has no old-Windows degradation path.
Every option call is checked; failure to apply a required request option fails the
request.

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

**Version-set semantics** (`supported_http_versions` -> options):

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

- **Automatic decompression.**
  `WinHttpSetOption(WINHTTP_OPTION_DECOMPRESSION, WINHTTP_DECOMPRESSION_FLAG_GZIP
  | WINHTTP_DECOMPRESSION_FLAG_DEFLATE)` makes WinHTTP advertise
  `Accept-Encoding: gzip, deflate`, transparently decode the response, and strip
  `Content-Encoding`/`Content-Length`.
- **Redirects.**
  `WINHTTP_OPTION_REDIRECT_POLICY = WINHTTP_OPTION_REDIRECT_POLICY_NEVER`, so
  redirect responses (3xx) are surfaced to the caller unchanged rather than
  followed.
- **Cookies and automatic authentication.** Each request sets
  `WINHTTP_OPTION_DISABLE_FEATURE` with `WINHTTP_DISABLE_COOKIES` (WinHTTP neither
  stores `Set-Cookie` nor auto-attaches `Cookie`) and
  `WINHTTP_OPTION_DISABLE_FEATURE` with `WINHTTP_DISABLE_AUTHENTICATION` (WinHTTP
  does not intercept 401/407 or attach credentials).
- **Idle connection reuse window.** `fetch`'s `connection_idle_timeout` (design.md §2.1)
  maps to the session-scoped `WINHTTP_OPTION_CONNECTION_IDLE_TIMEOUT`, applied during
  transport materialization before any connect or request handle is created, because
  WinHTTP rejects the option once a session has children. This option is absent from the
  public SDK, so the generated `windows` bindings do not define it and `bindings/mod.rs`
  declares it directly - the only constant in the crate that is not re-exported from
  those bindings. Its documentation there carries the contract the SDK does not publish,
  and is the reference for everything asserted in this bullet. One connection-manager
  field backs the idle sweep for HTTP/1.1, HTTP/2, and HTTP/3 alike, so this single value
  governs reuse eligibility for every protocol the transport negotiates; no per-protocol
  qualification applies. `Limited(d)` sets `d` rounded up to whole milliseconds, raised
  to WinHTTP's 5000 ms minimum when shorter - WinHTTP rejects a shorter window outright
  rather than clamping it, and a rejected option would fail session construction, so an
  aggressive idle policy must not become an unbuildable transport. `Unlimited` sets the
  largest representable `DWORD`, because the option's value is a plain unsigned
  millisecond count with no reserved "never expire" encoding. That window is finite - a
  little over 49 days - rather than a true infinity, which is why the design documents
  `Unlimited` as honored: the difference is not observable within a process lifetime.
  Setting the option also
  disables the process-wide keep-alive pool, which the session disables explicitly in
  any case, so the settings agree rather than conflict. The window bounds *reuse*:
  a connection past it is never selected again, because the lookup path re-checks the
  window. Sockets are released either by that same lookup, which tears down the expired
  connections it walks past, or by a periodic sweep for connections no lookup reaches -
  neither of which is prompt, so this is not a promise of timely socket release. Failure
  to apply the option leaves the materialized transport in its initialization-failure
  state, like every other required session option.
- **Keep-alive probes.** `fetch`'s `ConnectionKeepAlive` (design.md §2.1) maps to
  session-scoped `WINHTTP_OPTION_HTTP2_KEEPALIVE` /
  `WINHTTP_OPTION_HTTP3_KEEPALIVE` settings applied during transport
  materialization, before any connect or request handle is created. They make
  WinHTTP send an HTTP/2 or HTTP/3 PING once a connection has been idle for the
  configured interval, keeping pooled connections warm past the server's
  idle-close. Failure to apply either required session option leaves the
  materialized transport in its initialization-failure state. `Disabled` leaves
  the options unset. `interval` sets the option value, rounded up to whole
  milliseconds and clamped to the finite `DWORD` range. HTTP/2 values below WinHTTP's
  documented 5000 ms minimum are raised to 5000 ms. HTTP/3 has no documented minimum;
  values there are raised to a transport-chosen 1 ms floor. That floor exists only so a
  zero-length interval never reaches an option whose zero value WinHTTP leaves undefined:
  "keep-alive disabled" is a plausible reading of zero, and it would invert a caller that
  explicitly asked for probes. The fit is imperfect and is documented rather than
  diagnosed at runtime: WinHTTP PINGs idle pooled connections and has no separate
  "active-only" mode (so `ActiveConnections` and `ActiveAndIdleConnections` behave
  alike), it exposes a single
  interval and manages the probe-response timeout itself (so the `timeout` field is not
  separately honorable), and HTTP/1.1 has no application-level PING (so keep-alive there
  is plain TCP connection reuse, always on unless `WINHTTP_DISABLE_KEEP_ALIVE`, §9.3).

Every option above is part of the v1 implementation for the supported modern-Windows
baseline. The transport does not probe for alternatives or silently degrade when an
option call fails: required session-option failure creates the permanent failed handler
described in §3.2, and required request-option failure fails that request.

### 10.4 Timeout mapping

The timeout contract in design.md §6 is enforced at the highest layer that can observe
the required interval:

| `fetch` concept | Where enforced | WinHTTP timer configuration |
|-----------------|----------------|-----------------------------|
| Connect timeout | The request driver races connection establishment against `tick::Clock::delay(TransportOptions.connect_timeout)` | Native connect timer unlimited |
| Response timeout | `fetch::HttpClient::execute` wraps the whole pipeline through response headers | Native send/receive-response timers unlimited |
| Body idle timeout | `HttpBodyBuilder::body` wraps the returned body after merging request and client defaults | Native receive timer unlimited |
| Seatbelt request timeout | `seatbelt::TimeoutLayer` above the transport | n/a |
| Name resolution | Not separately bounded; it falls inside the connect timeout above | `WinHttpSetTimeouts` resolve field unlimited |

Session initialization explicitly programs the native resolve, connect, send, and receive
timers to their documented unlimited values rather than inheriting WinHTTP's finite
defaults. No additional native liveness backstop is installed.

`ResponseTimeout` needs no transport mapping: `fetch::HttpClient::execute` reads the
request extension and wraps the complete pipeline future, including WinHTTP connection
establishment, request upload, and response headers. `BodyTimeout` is read before the
response is built and copied into the per-call `HttpBodyOptions` passed to
`HttpBodyBuilder::body`; that builder merges client-level defaults and applies the
reset-on-frame idle timeout.

`TransportOptions.connect_timeout` is not wrapped by `fetch` processor, so the request driver
implements it with the supplied `tick::Clock`. This remains transport logic, but it does
not use a native WinHTTP timer.

Time conversions are option-specific because WinHTTP mixes signed and unsigned
millisecond fields with different unlimited sentinels. Configured finite timeouts and
probe intervals, including zero, clamp to at least 1 ms, except where a specific option
states a higher minimum. Positive sub-millisecond values
therefore round up to 1 ms. Values beyond a field's finite range clamp to its largest finite value.
HTTP/2 keep-alive intervals and the connection idle window both round up to WinHTTP's
5000 ms minimum when shorter, because WinHTTP rejects a shorter value for either rather
than clamping it.
These mechanical conversions do not warn or reject. Body lengths are never narrowed: each
`WinHttpWriteData` call is bounded to a `DWORD`, and larger bodies use multiple writes.
Other DWORD-valued options are handled individually when introduced rather than through a
generic lossy count conversion.

## 11. Handling generic options the transport cannot honor

`fetch`'s options arrive through its generic configuration surface, and callers set
them transport-agnostically, so the transport routinely receives settings it cannot
faithfully honor on WinHTTP - a `connection_lifetime` of `Fixed`/`PerConnection`
(design.md §2.2), a finite `max_connections` (design.md §2.1), and so on.

Unsupported generic options are ignored without warnings, counters, or build failures.
This includes generic `TlsOptions`, finite `max_connections`, connection lifetime
settings, and unrepresentable keep-alive semantics. Their behavior is documented in
design.md so callers can choose configuration appropriate to this transport.

These gaps are a symptom of `fetch`-level over-abstraction; the proper fix is
transport-level configuration (see the fetch API stabilization feedback,
../../fetch/docs/stabilization.md).

## 12. Telemetry

The transport reports through the `observed::Sink` supplied in its dependencies
(design.md §1.1). The emitted event, counter, and field names are the contractual
surface listed in design.md §8; this chapter covers how they are produced. The kinds of
signal emitted are deliberately distinct:

- **Metrics** (counters) stay low-cardinality: request count and error count. No
  per-request or per-connection attribute is attached to a metric.
- **Log events** may carry richer, higher-cardinality context that is useful for
  diagnosing a single failure but would be spam as a metric dimension.

**Session setup step identifiers.** The `winhttp.operation` field on the
initialization-failure event carries one of `open`, `set_timeouts`,
`disable_global_pooling`, `connection_idle_timeout`, `assured_non_blocking_callbacks`,
`http2_keep_alive`, `http3_keep_alive`, or `set_status_callback`. These name the session
setup calls made when a session is opened (§3.2) one for one, so adding or removing a
setup call changes
the set. That is why design.md §8 declares the field but not its values: pinning the
value set would freeze the internal sequence of setup calls into the crate's contract.

**Cold-connect error attribution (log only).** WinHTTP fires
`CONNECTING_TO_SERVER` when a request establishes a *new* physical connection
rather than reusing a pooled one. When a request then fails (connect timeout,
`REQUEST_ERROR`, `SECURE_FAILURE`), the transport annotates the failure's log event
with a marker that the failure occurred during a new connection attempt, plus the duration
measured from context installation to the point where the request headers were sent or
their send failed. Attribution turns on `CONNECTING_TO_SERVER` rather than
`CONNECTED_TO_SERVER`, so a request that fails while still connecting is attributed too;
that is the case an operator most needs to see. This lets an operator distinguish "the
server/pool is unhealthy" from "cold-connection establishment is slow or failing" - these
have different remediations. This attribution is attached only to the log event; it is
**not** promoted to a metric label, to keep connection-establishment noise out of the
metric cardinality.

## 13. Future opportunities

Design points deliberately deferred in v1, recorded here so they are revisited when
the `fetch` API or profiling data makes them actionable:

- **Consolidate per-processor sessions into one session per client.** v1 opens one WinHTTP
  session (and therefore one connection pool) per processor - and per `multiple_pools` slot
  within a processor (§3.2) - because that is the only shape the
  current `fetch` custom-transport API expresses while still isolating independently built
  clients (§3.2). This diverges from a single-session-per-client model: it trades
  cross-processor connection reuse and session-granularity connection recycling (the
  connection-lifetime control the transport does not offer today, design.md §2.2) for warmer,
  uncontended processor-local pools. Once `fetch` grows a per-built-client shared-state hook -
  or once profiling justifies `Isolation::Shared` despite processor-local object pools -
  revisit whether one shared session per client is the better default. Tracked as `fetch`
  API feedback (../../fetch/docs/stabilization.md, connection-management item) so the
  divergence is not forgotten when that API becomes more expressive.
- **Per-processor vs shared instancing as a knob.** Whether per-processor instancing is the right
  default at all could become configurable: a low-traffic client gains nothing from
  per-processor instances and might prefer a single shared instance (§3.2). Left unconfigurable
  in v1 - a knob earns its place only with demonstrated value - but a candidate if
  profiling shows it matters.
- **Session-keyed connection pools (`PoolIndex`).** v1 does not key anything on `fetch`'s
  `PoolIndex` value. It does not need to: `fetch` invokes the factory once per pool slot, so
  each slot already opens its own session/pool (§8), giving a resource profile of one
  session/pool per (processor × pool slot) rather than a single collapsed OS pool. If pool
  ownership stays a transport concern after the v2 sessions/pools discussion, revisit
  whether to interpret `PoolIndex` explicitly (../../fetch/docs/stabilization.md,
  connection-management item).
- **Per-connection identity through connection GUIDs.** WinHTTP exposes a pair of
  public request-scoped options, `WINHTTP_OPTION_CONNECTION_GUID` and
  `WINHTTP_OPTION_MATCH_CONNECTION_GUID`, that together give a connection an identity
  the transport currently assumes it cannot have: the first tags the connection serving
  a request with a caller-supplied GUID and reads back the GUID of the connection that
  served it, and the second steers a request onto a connection bearing a given tag,
  optionally forcing a new connection when no tagged match exists. Deferred design points
  become reachable with them, and each is a separate decision:
  - **`connection_lifetime` for `Fixed`/`PerConnection`** (design.md §2.2). Tagging
    connections with a generation GUID and rotating that generation on a schedule
    retires connections by age: requests demand the current generation, so a rotation
    forces fresh connections and the previous generation ages out through the idle
    timeout. A single rotating generation retires the whole pool at once, which is the
    synchronized-reconnect behavior `PerConnection` exists to prevent, so a faithful
    implementation needs several staggered generations rather than one.
  - **Cold-connect attribution on success.** §12 records connection establishment only
    on the failure path, so a client that silently reconnects on every request - the
    pathology an idle window that is too short produces - looks healthy in telemetry.
    Counting distinct connection GUIDs across requests measures it directly.
  - **`ConnectionInfo` on responses** (design.md §2.1). Recording when each GUID was
    first seen yields connection age, which is the input `fetch_hyper`'s
    `ConnectionInfo` reports and which this transport currently declares it does not
    track.
- **Distinguishing a near-zero idle window from "do not pool".** The idle window's
  5000 ms floor means a caller asking for a very short window gets five seconds, which is
  the mapping's largest semantic stretch: such a caller most plausibly means "do not pool
  at all", and §9.3 already documents the per-request
  `WINHTTP_OPTION_DISABLE_FEATURE | WINHTTP_DISABLE_KEEP_ALIVE` that expresses exactly
  that. Treating a sub-floor window as pooling-off would honor the intent more closely
  than raising it, but it reads intent into a duration rather than taking it at face
  value, so it needs a deliberate decision rather than an inference.
- **Native performance knobs.** WinHTTP carries further options covering socket send
  buffer sizing, TCP autotuning and initial retransmission timeout, allocation
  lookasides and heap extension, and response-body fast-forwarding that may bear on the
  read path in §6.2. None maps onto a generic `fetch` option, so adopting any would mean
  either a new transport-specific knob or an unconditional transport policy, and a knob
  earns its place only with demonstrated value. Recorded here so the option surface is known
  the next time profiling identifies a bottleneck rather than rediscovered then.
