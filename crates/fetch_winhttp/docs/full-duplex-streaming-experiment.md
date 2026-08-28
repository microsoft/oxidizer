# WinHTTP full-duplex request/response streaming experiment

This experiment determines whether native WinHTTP, on this host, can continue writing HTTP/2
request body data with `WinHttpWriteData` while the response headers/body are already being
received with `WinHttpReceiveResponse`/`WinHttpReadData` on the same request handle. Microsoft's
concurrency documentation is ambiguous by design:

- [Concurrency in WinHTTP](https://learn.microsoft.com/windows/win32/winhttp/concurrency-in-winhttp)
  states that "in some versions of Windows, the send and receive sides of a request are separate
  and may be used concurrently; an application may do a send-only operation on one thread at the
  same time that another thread is performing a receive-only operation," without stating which
  versions or what happens otherwise.
- [`WinHttpWriteData`](https://learn.microsoft.com/windows/win32/api/winhttp/nf-winhttp-winhttpwritedata)
  states that "when the application is sending data, it can call `WinHttpReceiveResponse` to end
  the data transfer," which reads as though receiving the response forecloses further writes.

The experiment therefore requires empirical evidence rather than a documentation reading: a
controlled local HTTP/2-over-TLS server that deliberately responds before the request body is
complete, and a client that tries every operation order the concurrency documentation could
plausibly justify.

## Method

The probe uses a self-signed `localhost` certificate (rustls server, `h2`-only ALPN) and three
cases, each against a fresh loopback listener:

1. **Baseline (sequencing control).** The server reads the entire two-chunk request body before
   responding, as an ordinary non-duplex handler would. This validates that the shared
   `ServerObservation` timestamps genuinely distinguish "responded before the final chunk" from
   "responded after it," rather than the duplex cases below being an artifact of how the harness
   measures time.
2. **Sequential interleave.** The duplex server observes the first request chunk, waits 200 ms,
   then sends response headers and a first response body chunk *before* the client sends its
   second (final) chunk. The client, on a single thread, writes the first chunk, calls
   `WinHttpReceiveResponse`, reads the first response chunk, and only then attempts a further
   `WinHttpWriteData` call for the second chunk. Every operation here fully completes before the
   next begins, so this case isolates whether `WinHttpReceiveResponse` itself ends the data
   transfer for a still-incomplete, known-length upload - independent of the multithreading
   question.
3. **Concurrent send-only/receive-only threads.** Against a second duplex server (400 ms response
   delay), the client writes the first chunk, then releases two threads through a
   `std::sync::Barrier`: one calls `WinHttpReceiveResponse` (a receive-only operation), the other
   calls `WinHttpWriteData` for the second chunk (a send-only operation) on the very same request
   handle. Both calls are timed; the case reports whether their active windows genuinely overlap in
   wall-clock time, not merely whether both calls returned successfully.

In every duplex case the server independently confirms delivery: it timestamps when it observes
each request chunk, when it hands the response to the HTTP/2 stack, and it re-reads a later
request chunk only after the response has already started flowing - all recorded in a shared
`Arc<Mutex<ServerObservation>>` that the client inspects directly (not inferred from client-side
call success). The client also verifies exact response/request byte content, not just status
codes. `WinHttpSetTimeouts` bounds every blocking WinHTTP call (5 s resolve/connect, 8 s
send/receive) and every server-side frame wait is wrapped in a `tokio::time::timeout` (5 s), so an
unsupported handle state surfaces as a bounded `ERROR_WINHTTP_TIMEOUT` rather than an indefinite
hang, and is distinguishable from an explicit rejection error.

HTTP/2 is required via `WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL` +
`WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED` in every case, and the negotiated protocol is asserted
after each response.

Run the probe on Windows:

```text
cargo +1.93.0 run -p fetch_winhttp --example full_duplex_streaming --all-features
```

### A pooling pitfall this experiment exposed

An earlier version of this probe hung indefinitely after a successful duplex exchange while
joining the server thread. The request and connect handles were dropped, but the *session*
handle was not: WinHTTP pools HTTP/2 connections at the session level for reuse (see
`implementation.md` section 9.1 on `WinHttpConnect`/session pooling), so the underlying TCP
connection stayed open and the server's `serve_connection` future never observed a clean
shutdown. Dropping the whole session (not just the request) after each case reproduces a clean
connection close. This is itself a useful, generalizable finding for anything that authors
short-lived WinHTTP integration probes: joining a server on connection closure requires closing
the session, not only the request/connect handles.

## Observed result

On a supported Windows host, all three cases pass and the probe exits successfully:

```text
baseline (sequencing control): status=200, protocol=1, response body="response-chunk-a"
sequencing control confirmed: non-duplex handling responds only after the full request body arrives.

sequential: status=200, protocol=1, response observed before the final chunk was sent (server chunk2 not yet seen, response already sent).
sequential: WinHttpWriteData(chunk2) succeeded while the response was already flowing.
sequential: server confirmed receiving the second chunk after already responding.

concurrent: receive-only WinHttpReceiveResponse active for 402.1833ms (result="Ok"); send-only WinHttpWriteData(chunk2) active for 249.3µs (result="Ok"); overlapping=true
concurrent: status=200, protocol=1
concurrent: server confirmed receiving the second chunk after already responding.

DECISIVE: after WinHttpReceiveResponse observed headers and a response body chunk for a still-incomplete upload, a further WinHttpWriteData call on the same request handle succeeded on a single thread (sequential interleave). Full-duplex request/response streaming is supported on this host.
```

`protocol=1` is `WINHTTP_PROTOCOL_FLAG_HTTP2` in every case. The result was reproduced across five
consecutive runs with identical outcomes (the ~400 ms receive-only window and ~200 µs send-only
window in the concurrent case varied by tens of microseconds between runs but never lost the
`overlapping=true` result).

Two independent, decisive facts follow from this:

1. **`WinHttpReceiveResponse` does not, by itself, end the data transfer for a still-incomplete,
   known-length (`dwTotalLength`-declared) HTTP/2 upload on this host.** The client observed
   response headers and a response body chunk, then successfully wrote and completed the
   remaining upload on the very same request handle from the very same thread - the simplest
   possible operation order. This directly resolves the ambiguity in the `WinHttpWriteData`
   remark: that remark describes callers who choose to stop uploading and finalize early, not a
   statement that receiving forecloses further sends in general. The corrected unknown-length
   cases below establish the same result for automatic-chunking uploads.
2. **A send-only `WinHttpWriteData` call and a receive-only `WinHttpReceiveResponse` call on the
   same request handle from two different threads genuinely overlap in wall-clock time and both
   succeed**, directly confirming the concurrency documentation's "in some versions of Windows"
   claim holds on this host. This case was not required to reach the decisive result above (the
   single-thread sequential case already succeeded), but it independently corroborates the same
   conclusion through the documented concurrent code path.

No case in this probe produced a Win32-level rejection; there was no "sequential fails, escalate
to concurrent threads" branch to exercise on this host, because the simplest order already
succeeded.

## Documented surface and remaining uncertainty

- [`WinHttpSendRequest`](https://learn.microsoft.com/windows/win32/api/winhttp/nf-winhttp-winhttpsendrequest)
  documents `dwTotalLength` as a fixed value that "must not change between calls," used here as
  the two chunks' combined length so WinHTTP can track completion without chunked encoding.
- [`WinHttpReceiveResponse`](https://learn.microsoft.com/windows/win32/api/winhttp/nf-winhttp-winhttpreceiveresponse)
  and [`WinHttpWriteData`](https://learn.microsoft.com/windows/win32/api/winhttp/nf-winhttp-winhttpwritedata)
  do not document a success/failure contract for writing more data after receiving the response
  headers of a still-incomplete upload; this probe fills that gap empirically for one Windows
  build.
- [Concurrency in WinHTTP](https://learn.microsoft.com/windows/win32/winhttp/concurrency-in-winhttp)
  explicitly scopes the send/receive concurrency exception to "some versions of Windows" without
  naming them, and does not document what happens on versions where it does not apply (a
  synchronous error, silent internal serialization, or something else).

This probe ran on Windows build 26100.9106 (Windows 11, version 24H2; the registry's
`ProductName` value on this host reports "Windows 10 Enterprise N", a known cosmetic artifact of
that key not being updated for Windows 11 - the build number is authoritative). The result is
empirical and version-specific:

- It is not known whether earlier Windows 10 builds, other Windows 11 builds, or Windows Server
  builds preserve this behavior, silently serialize the concurrent case without erroring, or
  reject the sequential/concurrent write with a Win32 error such as
  `ERROR_WINHTTP_INCORRECT_HANDLE_STATE` or `ERROR_WINHTTP_CONNECTION_ERROR`.
- The probe's diagnostics (exact Win32 error codes via `WinHttpError`, negotiated protocol,
  server-observed byte content and timestamps, and the overlap computation in the concurrent
  case) are designed to make that distinction unambiguous if re-run on a different build: a
  recorded Win32 error means the order is rejected on that host; a bounded `ERROR_WINHTTP_TIMEOUT`
  after full send/receive timeouts elapse means the operation hung rather than failed fast; and a
  successful write whose duration matches the peer's response delay (rather than completing
  quickly) would indicate silent internal serialization rather than genuine concurrency.
- Anything that depends on this capability in production should not assume it holds universally
  across the Windows fleet without a compatibility check or a runtime capability probe, and should
  retain an integration test (mirroring this probe) to catch a regression if a future Windows
  update changes the behavior.

## Unknown-length (`WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH`) request uploads

The known-length result above answers a real question, but gRPC/client-streaming uploads do not
know their total size up front: a gRPC client stream sends an unbounded number of messages and
only "half-closes" the request when it decides it has no more to send. The same probe binary
therefore also determines what happens when the request body's total length is unknown.

### A previous version of this probe used the wrong API and was invalid

An earlier version of this probe combined the documented `WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH`
sentinel with a manually added `Transfer-Encoding: chunked` header added via
`WinHttpAddRequestHeaders`, following
[Microsoft's guidance for `WinHttpSendRequest`](https://learn.microsoft.com/windows/win32/api/winhttp/nf-winhttp-winhttpsendrequest)
read literally as the HTTP/1.1 chunked-transfer idiom. **That combination is not the API
`fetch_winhttp_impl` uses in production, and it produced a false negative result:**
`WinHttpSendRequest` rejected `WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED` alongside the header outright
(Win32 error 12190, `ERROR_WINHTTP_HTTP_PROTOCOL_MISMATCH`), and with only
`WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL` set, exactly one `WinHttpWriteData` call ever succeeded
before every later operation failed with `ERROR_WINHTTP_INVALID_SERVER_RESPONSE` (12152). **Those
results are superseded by the corrected probe below and must not be read as evidence that WinHTTP
cannot support unbounded, unknown-length HTTP/2 uploads** - they only show that this particular,
incorrect way of asking for one does not work.

`fetch_winhttp_impl` (`crates/fetch_winhttp_impl/src/body/write.rs`'s `RequestBodyFraming` and
`WinHttpBodyWriter`, and `crates/fetch_winhttp_impl/src/request.rs`'s request lifecycle, as landed
by PR #687) instead:

- opens the request handle with `WINHTTP_FLAG_AUTOMATIC_CHUNKING` set on `WinHttpOpenRequest`
  (`convert.rs`'s `request_open_flags`) whenever the body reports no length and no `Content-Length`
  header is present;
- passes `WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH` for `dwTotalLength` to `WinHttpSendRequest`, exactly
  as the earlier, invalid probe did;
- **never adds a `Transfer-Encoding` header** - `RequestBodyFraming::new` rejects one outright if
  the caller supplies it, because `WinHTTP` performs the chunked framing itself once
  `WINHTTP_FLAG_AUTOMATIC_CHUNKING` is set, and forwarding a caller-supplied transfer coding next
  to `WinHTTP`'s own framing is the classic request-smuggling primitive (RFC 9112 §6.1);
- ends the body with a single, final `WinHttpWriteData` call whose buffer pointer is `NULL` and
  whose length is `0` (`WinHttpBodyWriter::end_automatic_chunking`) - not a zero-length write over
  a valid-but-empty buffer pointer - and awaits its completion before calling
  `WinHttpReceiveResponse`.

PR #687's own `http2_streams_unknown_length_uploads_and_preserves_response_trailers` integration
test drives exactly this combination - `WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED` set (because its
test client's supported versions are `[Version::HTTP_2]` only, and `protocol_options` requires the
protocol whenever HTTP/1.1 is excluded) together with `WINHTTP_FLAG_AUTOMATIC_CHUNKING` - against a
real local WinHTTP connection, and it passes. This probe reproduces that exact native
flag/header/total-length lowering directly, so it can also exercise the sequential/concurrent
duplex reordering PR #687's own test does not attempt.

### Method

The corrected unknown-length cases mirror the known-length baseline/sequential/concurrent trio
above, with two API-level differences: the request handle is opened with
`WinHttpOpenRequest(..., WINHTTP_FLAG_SECURE | WINHTTP_FLAG_AUTOMATIC_CHUNKING)` instead of
`WINHTTP_FLAG_SECURE` alone, and `WinHttpSendRequest` receives `WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH`
instead of the two chunks' combined length. `WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED` is still set in
every case, matching every known-length case above and PR #687's own required-HTTP/2 test.

1. **Baseline (sequencing control).** Identical in shape to the known-length baseline: the server
   reads the entire request body - including the null-buffer terminal write - before responding.
   This proves the corrected native API completes an HTTP/2-required, unknown-length upload
   end-to-end, with no `Transfer-Encoding` header and no protocol-mismatch error, before the duplex
   cases below reorder it.
2. **Sequential interleave.** The duplex server responds after the first chunk, exactly as the
   known-length sequential case does. The client writes the first chunk, calls
   `WinHttpReceiveResponse` and reads the first response chunk *before* the upload is complete -
   before the second chunk or the null-buffer terminal write have been sent - then writes the
   remaining chunk and performs the terminal write on the same thread.
3. **Concurrent send-only/receive-only threads.** Direct analogue of the known-length concurrent
   case: a receive-only thread calls `WinHttpReceiveResponse` while a send-only thread writes the
   remaining chunk and the null-buffer terminal write, released through the same `Barrier` pattern
   and timed for genuine overlap the same way.

Every case still bounds every blocking `WinHTTP` call via `WinHttpSetTimeouts` and every
server-side frame wait via a `tokio::time::timeout`, exactly as the known-length cases do.

Run the probe on Windows (the same binary covers both the known-length and unknown-length cases):

```text
cargo +1.93.0 run -p fetch_winhttp --example full_duplex_streaming --all-features
```

### Observed result

On the same Windows host, the corrected unknown-length cases are positive and fully reproducible
across five consecutive runs with identical results every time:

```text
unknown-length baseline (sequencing control): status=200, protocol=1, response body="response-chunk-a"
unknown-length sequencing control confirmed: WINHTTP_FLAG_AUTOMATIC_CHUNKING + WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH complete an HTTP/2-required upload end-to-end with no Transfer-Encoding header.

unknown-length sequential: status=200, protocol=1, response observed before the final chunk was sent (server chunk2 not yet seen, response already sent).
unknown-length sequential: WinHttpWriteData(chunk2) succeeded while the response was already flowing.
unknown-length sequential: the null-buffer terminal write succeeded, ending the automatically chunked upload.
unknown-length sequential: server confirmed receiving the second chunk after already responding.

unknown-length concurrent: receive-only WinHttpReceiveResponse active for 404.9356ms (result=Ok); send-only WinHttpWriteData(chunk2)+terminal active for 399.4µs (chunk2=Succeeded, terminal=Succeeded); overlapping=true
unknown-length concurrent: status=200, protocol=1
unknown-length concurrent: server confirmed receiving the second chunk after already responding.

DECISIVE (unknown length): after WinHttpReceiveResponse observed headers and a response body chunk for a still-incomplete WINHTTP_FLAG_AUTOMATIC_CHUNKING upload (no Transfer-Encoding header, dwTotalLength=WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH, HTTP/2 required), a further WinHttpWriteData call and the documented null-buffer terminal write both succeeded on the same request handle from the same thread (sequential interleave). True unbounded, gRPC-style full-duplex request/response streaming is supported on this host using the same native automatic-chunking API PR #687 uses.
```

The receive-only window (~400 ms) and send-only window (hundreds of microseconds) in the
concurrent case varied by tens of microseconds between runs, exactly as in the known-length
concurrent case, but `overlapping=true` and every step succeeded identically every time.

Two independent, decisive facts follow, directly correcting the previous, invalid probe's
conclusions:

1. **`WINHTTP_FLAG_AUTOMATIC_CHUNKING` combined with `WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED` is not
   mutually exclusive**, unlike the manually added `Transfer-Encoding: chunked` header the earlier
   probe used. `WinHttpSendRequest` accepts `WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH` on an
   automatically chunked, HTTP/2-required request without error, and the request negotiates HTTP/2
   as required.
2. **`WinHttpReceiveResponse` does not end an unknown-length upload's data transfer just because
   the automatic-chunking terminal write has not been sent yet**, mirroring the known-length
   result. The client observed response headers and a response body chunk for a still-incomplete,
   automatically chunked upload, then successfully wrote the remaining chunk and the documented
   null-buffer terminal write on the same request handle - both sequentially on one thread and
   concurrently across a genuinely overlapping send-only/receive-only thread pair. Native WinHTTP
   therefore does support true, unbounded gRPC-style full-duplex request/response streaming, using
   the same `WINHTTP_FLAG_AUTOMATIC_CHUNKING` + `WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH` API PR #687's
   `fetch_winhttp_impl` uses in production - the earlier probe's negative conclusion was an
   artifact of using the wrong native API, not a genuine limitation of WinHTTP or of HTTP/2 itself.

## Design implications for gRPC-style duplex support over WinHTTP

- **True, unbounded gRPC/client- and bidi-streaming is supported over native WinHTTP** through
  `WINHTTP_FLAG_AUTOMATIC_CHUNKING` (set on `WinHttpOpenRequest`) combined with
  `WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH` (passed to `WinHttpSendRequest`), with no
  `Transfer-Encoding` header ever added by the caller - exactly the API `fetch_winhttp_impl`'s
  `RequestBodyFraming` and `WinHttpBodyWriter` use (PR #687). **This supersedes this document's
  earlier conclusion**, which was reached with a manually added `Transfer-Encoding: chunked` header
  instead of the automatic-chunking flag and found the opposite (negative) result; that earlier
  finding was a consequence of using the wrong native API, not a real limitation.
- **The known-length result remains useful as a fallback for other backends or older Windows
  builds**, but a WinHTTP-backed `fetch` streaming-body implementation does not need to fall back
  to a concrete `dwTotalLength` ceiling to support unbounded uploads: `WINHTTP_FLAG_AUTOMATIC_CHUNKING`
  gives it a genuinely unknown-length path with the same full-duplex behavior this probe already
  proved for known-length uploads.
- **This result is empirical and host/version-specific**, exactly like the known-length result
  above: it is not known whether other Windows builds preserve the same behavior, and any
  `fetch_winhttp` implementation that relies on it should retain an integration test (mirroring
  PR #687's `http2_streams_unknown_length_uploads_and_preserves_response_trailers`, and ideally this
  probe's duplex reordering) to catch a regression if a future Windows update changes it.

## Documented surface and remaining uncertainty (unknown length)

- [`WinHttpOpenRequest`](https://learn.microsoft.com/windows/win32/api/winhttp/nf-winhttp-winhttpopenrequest)
  documents `WINHTTP_FLAG_AUTOMATIC_CHUNKING` only as enabling "automatic chunked transfer encoding
  ... when the exact content length is not known," with no explicit statement of its interaction
  with a negotiated or required HTTP/2 connection, nor with `WinHttpReceiveResponse` being called
  while the automatically chunked upload is still open; this probe fills that gap empirically for
  one Windows build.
- [`WinHttpSendRequest`](https://learn.microsoft.com/windows/win32/api/winhttp/nf-winhttp-winhttpsendrequest)
  documents `WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH` only by name and value (`0`), with the same gap.
- [`WinHttpWriteData`](https://learn.microsoft.com/windows/win32/api/winhttp/nf-winhttp-winhttpwritedata)
  does not explicitly document that a `NULL` buffer paired with a zero length is how an
  automatically chunked upload ends, as opposed to a zero-length write over a valid (if empty)
  buffer pointer; `fetch_winhttp_impl`'s `WinHttpBodyWriter::end_automatic_chunking` uses the
  `NULL`-buffer form, and this probe reproduces that exact call rather than testing whether the two
  forms are equivalent.
- This probe ran on the same Windows build 26100.9106 (Windows 11, version 24H2) as the
  known-length probe above, immediately afterward in the same process, so all results in this
  document share an identical environment. It is not known whether other Windows builds preserve
  this exact behavior, silently serialize the concurrent case without erroring, or reject any of
  these calls with a Win32 error - the same open question the known-length result above already
  carries.
