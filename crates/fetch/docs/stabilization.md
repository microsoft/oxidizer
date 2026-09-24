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

Some items below also carry **reviewer input** captured during the `fetch_winhttp`
review (labeled as such). These notes aggregate ideas and options for the stabilization
phase to analyze later; they are not agreed decisions, and the actual direction will be
settled separately, outside this PR.

The items below should be resolved (or consciously accepted) before stabilization:

## 1. First-class transport plug-in

There is no first-class way to plug in a downstream transport. `HttpClient` and its
builder live in `fetch`, and the orphan rule forbids a downstream crate from adding
inherent constructors, so `fetch_winhttp` ships an extension trait
(`HttpClientWinHttpExt`) the caller must import to get `HttpClient::builder_winhttp`.
That works, but it is asymmetric: transports bundled in `fetch` get
first-class inherent methods (`HttpClient::builder_tokio`/`new_tokio`), while a
downstream transport is reachable only through an imported trait. The extension trait is a
workaround, not a fix - it papers over the call syntax but leaves the asymmetry, and the
asymmetry itself is the wart: all HTTP-client composition options should be on equal
footing for good UX. A transport-agnostic
API where the caller always explicitly constructs and plugs in a transport - e.g.
`HttpClient::build(fetch_winhttp::transport().tls(cfg))` - would be more predictable,
avoid a Hyper-colored default, and treat every transport uniformly. The slightly more
verbose hello-world is worth the consistency.

**Reviewer input (collected for stabilization, not a decision).** A counter-view holds
that `fetch` is deliberately *batteries-included* and so should *play favorites*: pick a
recommended default transport per supported runtime and keep its hello-world maximally
terse, so the 90% of users who just want the default runtime pay no extra ceremony, while
custom transports remain possible without degrading the default ergonomics. One shape
suggested for this: `HttpClient::builder()` returns `HttpClientBuilder<NeedsTransport>`
(exposing only transport-agnostic pipeline configuration) and a transport must be plugged
in to reach a buildable state, while `HttpClient::builder_tokio(...)` returns
`HttpClientBuilder<DefaultTransport>` (the recommended transport pre-plugged); the generic
is defaulted (`HttpClientBuilder<T = DefaultTransport>`) and kept off `HttpClient` itself.
Deeper customization of the default transport could still be exposed, e.g.
`HttpClient::builder(...).hyper(hyper_transport)`. The current transport bundling is also
viewed as a stopgap pending runtime/security stabilization, after which built-in
`builder_oxidizer`/`new_oxidizer` constructors would arrive.

**Reviewer input (collected for stabilization, not a decision).** A further perspective
questions whether the transport list needs to be open-ended at all. `fetch` could bundle
`fetch_winhttp` directly: the open extensibility exists today mainly because an internal
`fetch` variant lives out-of-tree, and once that variant moves into this repository the
requirement for downstream transports may go away. The transport/pipeline split still holds
regardless. A related crate-layering idea (mirroring how .NET ships WinHTTP as an optional
`System.Net.Http.WinHttpHandler` package rather than inside the core client) keeps `fetch`
an *enabler with reasonable defaults* rather than a monolithic all-in-one crate: split out
a `fetch_core` crate that excludes the transport layer, and have `fetch` re-export it plus a
set of supported transports. Libraries would depend only on `fetch_core`; applications would
depend on `fetch`. This keeps the heavy transport plumbing (WinHTTP's especially) out of the
dependency graph of code that does not need it.

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

**Reviewer input (collected for stabilization, not a decision).** An alternative to
moving TLS entirely onto each transport is to keep a `fetch`-level TLS type as a set of
transport-agnostic *descriptors* that each TLS technology interprets in its own way; the
open problem there is representing capabilities that some backends expose and others do
not (for example certificate-validation callbacks exist for rustls but not native-tls or
Schannel). A related construction idea is to require TLS explicitly at construction rather
than threading it through the builder, e.g. `HttpClient::builder_tokio(deps, tls)`.

The silent-ignore hazard is what makes this pressing: because `CustomContext::tls` is
always populated, a caller who sets security-affecting `TlsOptions` (a private CA root, a
client identity, cert pinning) and plugs in a transport that cannot consume them - like
`fetch_winhttp` - gets a default system-trust connection with no signal, even though the
same code is honored by `fetch_hyper`. Rather than have each transport special-case,
warn, or reject on every such field (playing whack-a-mole with a mis-shaped API), the fix
belongs in the TLS-configuration redesign above: TLS material should be configured on the
transport that can honor it, so an unhonorable combination is unrepresentable rather than
silently dropped.

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

**Reviewer input (collected for stabilization, not a decision).** The pool model itself
is unresolved: `fetch` today exposes `multiple_pools`/`PoolIndex` and invokes a custom
transport's factory once per pool slot, presuming `fetch` owns pool partitioning. A
transport like WinHTTP owns its own connection pool and does not key anything on the
externally supplied `PoolIndex` value (see fetch_winhttp implementation.md §8); because
`fetch` calls the factory once per slot, each slot opens its own WinHTTP session and pool,
so nominally separate pools do stay separate - the resource profile is one session/pool per
(thread × pool slot), not a single collapsed pool. The open question is ownership: since
connection management generally cannot be generalized across transports, pool partitioning
most likely belongs on the transport layer, which may retire the `PoolIndex` surface in its
current shape. Where pool management lives should be settled as part of the v2 "what do we
do about sessions/pools" discussion.

## 4. Scope-based timeout configuration

Timeouts are over-abstracted, not under-modeled. `fetch` models a connect
timeout but has no concept for resolve or send timeouts. That absence is *not* the
problem: different transports support different sets of fine-grained timers, measure
them against different phase boundaries (what each timer includes or excludes), or
cannot express some of them at all. Per the scope split above, *network-phase* timers
(resolve, connect, send, receive) are transport-specific: which of them a transport can
express, and against which phase boundaries, is its own concern (see fetch_winhttp
design.md §6.1). The mismatch
today is that `fetch` reaches down to model a connect timeout while leaving the rest to
transports - it should leave all network-phase timers to the transport and keep only
pipeline-level deadlines.

**Reviewer input (collected for stabilization, not a decision).** A refinement of the
split: even if fine-grained network-phase timers move to transports, *connect timeout* is
the one timeout end consumers care about most, so it is worth keeping as a common,
transport-agnostic knob that each transport interprets in its own terms. The transport's
last-resort mechanism for any deadline it cannot express natively is drop safety - when
`fetch` cancels the request future, the transport must honor it and tear down promptly
(see fetch_winhttp design.md §6, §7).

## 5. Transport construction ergonomics

Constructing a client from a transport is clumsy. Because a transport's configuration
arrives as a single deps value, the caller assembles that value separately and passes the
result in -
`HttpClient::builder_winhttp(WinHttpDeps::builder(clock, pool, sink).tls(cfg).build())` -
rather than configuring the transport as part of one fluent chain. A more natural surface
would configure the transport through closures on the builder, e.g.
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

## 7. Application-vs-library configuration responsibility

Beyond *where* a knob lives (transport vs pipeline, per the split above), stabilization
must settle *who* sets it when `fetch` is consumed indirectly. This is the ownership
dimension of the same layering question and it intersects item 3's connection-management
split.

**Reviewer input (collected for stabilization, not a decision).** Two usage shapes pull in
different directions:

- An application calling an API directly controls everything and needs the full knob set
  (transport choice, TLS, cert validation, connection lifetime, resilience policy). This is
  the simple case - the application makes every decision.
- A library calling endpoints on the application's behalf (e.g. the ECS use case) splits
  that responsibility: some configuration is the library's to own, some the application's.

Three models were considered for the indirect case: (a) the application configures nothing
and the library owns the whole client - maximally free for the library, but the application
cannot work around misconfiguration and inherits whatever transport/TLS dependencies the
library picks (e.g. a forced rustls in the tree); (b) the application configures and owns
the whole `HttpClient` and hands the library a finished client - but then the library loses
control over things it should arguably own, such as its retry policy; (c) a mix where the
application supplies the transport, the library configures the pipeline concerns it cares
about, and the application may still override selected properties. The mix (c) is seen as
the most ergonomic, with a workable fallback being (b) plus a library-exposed
"configure this `HttpClientBuilder`" function (considered a UX wart).

The obstacle to (c) is that the configuration libraries care about does not fall cleanly on
the pipeline side of the transport/pipeline split: connection lifetime, for instance,
matters deeply to ECS yet is a transport concern. Early analysis suggests the knobs
libraries most commonly want can be honored by both the Hyper and WinHTTP transports, but
only if the transport abstraction is richer than a plain layered/Tower service.

## 8. Transport-specific protocol and pool tuning still on the shared surface

`ConnectionPoolOptions`, `Http2Options`, and `ConnectionKeepAlive` remain on
`TransportOptions`, although their current behavior is implemented by `fetch_hyper` and
cannot be guaranteed by every transport.

`SocketOptions` belongs to `TokioTransportOptions`, where only the bundled socket-owning
transport accepts it.

No bundled transport currently reads `TransportOptions::extra`.
