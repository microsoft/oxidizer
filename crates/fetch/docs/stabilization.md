# `fetch` API stabilization feedback

Building the `fetch_winhttp` transport (a Windows-only WinHTTP-based `fetch`
transport) surfaced several places where the current `fetch` API is shaped around its
original Hyper/Tokio transport and makes assumptions that do not hold for every
transport. None blocks that transport, but each is worth addressing before the `fetch`
API is stabilized.

The common thread is a layering question: `fetch` is fundamentally a *pipeline
assembler* (it builds the HTTP request/response pipeline and its middleware), while
the *transport* is the component that actually performs network communication.
Configuration should be split along that line - anything that governs how bytes
travel over the network (TLS, the network-phase timeouts, connection lifetime and
reuse) is a transport concern and belongs on the transport, while end-to-end pipeline
concerns (overall response deadline, retry, telemetry) stay in `fetch`. Several of
the items below are instances of that split being in the wrong place today.

The items below should be resolved (or consciously accepted) before stabilization:

## 1. First-class transport plug-in

There is no first-class way to plug in a downstream transport. `HttpClient` and its
builder live in `fetch`, and the orphan rule forbids a downstream crate from adding
inherent constructors, so `fetch_winhttp` ships an extension trait
(`HttpClientWinHttpExt`) the caller must import to get `HttpClient::builder_winhttp`/
`new_winhttp`. That works, but it is asymmetric: transports bundled in `fetch` get
first-class inherent methods (`HttpClient::builder_tokio`/`new_tokio`), while a
downstream transport is reachable only through an imported trait. The extension trait is a
workaround, not a fix - it papers over the call syntax but leaves the asymmetry, and the
asymmetry itself is the wart: all HTTP-client composition options should be on equal
footing for good UX. A transport-agnostic
API where the caller always explicitly constructs and plugs in a transport - e.g.
`HttpClient::build(fetch_winhttp::transport().tls(cfg))` - would be more predictable,
avoid a Hyper-colored default, and treat every transport uniformly. The slightly more
verbose hello-world is worth the consistency.

## 2. Transport-specific TLS configuration

TLS configuration is over-abstracted. `fetch`'s generic `TlsOptions`
carries rustls/native-tls material that only `fetch_hyper` - the Hyper-based `fetch`
transport, which configures its TLS through exactly that material - can consume;
Schannel-based `fetch_winhttp` cannot use it and needs its own knobs (see fetch_winhttp design.md §1.2 and §4).
The TLS model is a property of each transport, not of `fetch`: `fetch_hyper` and
`fetch_winhttp` have fundamentally different, non-interchangeable TLS configuration, so
it cannot be handled at the `fetch` layer at all. Per the split above, TLS should be
configured per transport, on the transport being plugged in; the
`HttpClient::build(transport().tls(cfg))` shape would express this cleanly.

## 3. Transport-specific connection management

Connection-management options are over-abstracted. `fetch` exposes
`max_connections`, `connection_idle_timeout`, `connection_lifetime`, and
`ConnectionKeepAlive` (see fetch_winhttp design.md §2.1), but
different transports pool and manage connections differently, so how (and whether)
each option can be honored is entirely transport-dependent. `max_connections` is the
clearest example: it presumes a `fetch`-managed connection pool, yet WinHTTP owns its
own pool with its own limit knobs, and a transport over a different stack would model
concurrency differently again. The other options are the same story: WinHTTP exposes no
per-connection age control, so `connection_lifetime` cannot be implemented here at all
(see fetch_winhttp design.md §2.2), and it applies its own per-protocol keep-alive with
its own floors, so `ConnectionKeepAlive` and `connection_idle_timeout` map only
approximately or not at all. Per the split above, this configuration belongs on the
transport.

## 4. Scope-based timeout configuration

Timeouts are over-abstracted, not under-modeled. `fetch` models a connect
timeout but has no concept for resolve or send timeouts. That absence is *not* the
problem: different transports support different sets of fine-grained timers, measure
them against different phase boundaries (what each timer includes or excludes), or
cannot express some of them at all. Per the scope split above, *network-phase* timers
(resolve, connect, send, receive) are transport-specific, which is why this transport
exposes them as `WinHttpOptions` knobs (see fetch_winhttp design.md §6.1). The mismatch
today is that `fetch` reaches down to model a connect timeout while leaving the rest to
transports - it should leave all network-phase timers to the transport and keep only
pipeline-level deadlines.

## 5. Transport construction ergonomics

Constructing a client from a transport is clumsy. Because a transport's configuration
arrives as a single deps struct, the caller writes a struct literal -
`HttpClient::builder_winhttp(WinHttpDeps { tls: ..., options: ..., sink: ... })` - rather
than a fluent chain. A more natural surface would configure the transport through closures
on the builder, e.g.
`HttpClient::builder_winhttp().tls(|tls| tls.accept_invalid_certs()).timeouts(|t| t.read(3000)).custom_pipeline(..).build()`,
or a compositional form where the transport is itself a builder plugged into the client:
`HttpClient::builder().transport(WinHttpTransport::builder().tls(|tls| ..).build()).build()`.
Either reads more naturally than a deps struct and scales better as transports gain knobs.
This is the ergonomic dimension of item 1's plug-in surface - the two should be resolved
together. Acceptable for v1, but worth revisiting before stabilization.

## 6. Telemetry sink should be a `fetch`-provided dependency

The transport needs a telemetry sink (an `observed::Sink`) to emit its events and metrics,
and today it has to accept one in its own deps (`WinHttpDeps::sink`) because `fetch` does
not provide one. Telemetry is not a transport-specific concern: every transport needs the
same sink, and `fetch` already owns the client's telemetry meter. `fetch` should hand the
sink to the transport the same way it hands over the clock and memory pool (through the
custom-transport context), so no transport has to surface a sink of its own. Until then,
`fetch_winhttp` carries it in `WinHttpDeps`.
