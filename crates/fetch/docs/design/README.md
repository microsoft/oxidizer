# `fetch` design

`fetch` provides a stable HTTP client and configuration surface over transports with different
capabilities. Applications choose a transport, while libraries can configure the portable
networking behavior they require without knowing which transport the application selected.

The crate is independent of concrete HTTP and TLS implementations. Applications select focused
transport composition crates; libraries that use only portable requirements depend on `fetch`
alone.

## Configuration model

Configuration is classified by semantics and ownership:

1. **Pipeline policy** is implemented above the transport and is available for every client.
   Routing, resilience, telemetry, redaction, response policy, and custom middleware are in this
   category.
2. **Portable transport requirements** describe observable networking behavior that a library or
   application may require. Examples include connection lifetime, connection limits, protocol
   constraints, certificate authentication, and portable trust policy. Every transport must honor
   an explicit requirement or reject client construction.
3. **Transport-specific configuration** controls mechanisms that have no portable contract.
   Applications normally set these options on a composition builder before passing the resulting
   transport to `fetch`. A library that deliberately supports a particular transport setting may
   use its independently versioned, dependency-light configuration crate through the builder's
   typed configuration registry. Backend-typed mechanisms such as rustls verifier callbacks remain
   on the backend composition crate. A mechanism does not automatically deserve public
   configuration; routine flow-control, socket-buffer, and congestion tuning remains
   transport-owned.

The fact that a behavior is implemented by a transport does not make it transport-specific.
Connection lifetime is implemented differently by Hyper and WinHTTP, but its useful contract can
be expressed portably. Conversely, a rustls verifier callback is transport-specific because its
contract is the rustls callback API itself.

Detailed requirement semantics and lowering rules are defined in
[transport configuration](transport-configuration.md). The current backend comparison and the
public-surface conclusion are in the [capability matrix](capability-matrix.md).

## Composition across application and library boundaries

`HttpClient` and `HttpClientBuilder` are concrete, transport-erased types. There are no partial
portable capability profiles: every supported transport, including external transports and test
fakes, implements the complete library-facing baseline. Retaining the transport type in the builder
would therefore add generic complexity without preventing a demonstrated incompatibility.

The application configures transport-specific behavior before handing the transport to `fetch`:

```rust,ignore
let transport = fetch::transport::winhttp()
    .proxy(proxy)
    .integrated_authentication(true);

let builder = fetch::HttpClient::builder(transport);
let client = service_library::build_client(builder)?;
```

The transport-independent path does not name Hyper, rustls, native TLS, or WinHTTP:

```rust,ignore
pub fn build_client(
    builder: fetch::HttpClientBuilder,
) -> fetch::Result<fetch::HttpClient> {
    builder
        .client_certificate(fetch::ClientCredentialId::new("tvs-client"))
        .tls_server_name(
            fetch::Origin::https("localhost", service_port),
            fetch::ServerName::new("tvs.prod.example")?,
        )
        .connection_lifetime(Duration::from_minutes(30))
        .standard_pipeline(configure_resilience)
        .build()
}
```

This preserves the important ownership split:

- the application chooses Hyper with rustls, Hyper with native TLS, WinHTTP, or a fake;
- the library declares the behavior its service requires;
- every accepted transport implements the full library-facing contract;
- `fetch` validates requested values, credential bindings, and host support during construction;
- neither party silently overrides or weakens the other's requirements.

A library that requires no configuration accepts a built `HttpClient`. A library that owns
pipeline or portable transport policy accepts an `HttpClientBuilder` and builds the concrete
client after applying its requirements.

### Transport configuration registry

The erased builder carries a type-indexed configuration registry until `build`. A selected
transport registers the dependency-light configuration types it supports, and a library may query
or mutate one without depending on the transport implementation:

```rust,ignore
let winhttp = builder
    .transport_config_mut::<fetch_winhttp_config::WinHttpOptions>()
    .ok_or(BuildError::WinHttpRequired)?;

winhttp.use_integrated_proxy_discovery(true);
```

Configuration companion crates contain data and policy types only. They do not depend on Hyper,
WinHTTP FFI, a TLS implementation, or a crypto provider. Their versions and stability promises are
independent of `fetch` and the transport implementation. Presence identifies support; absence is
either ignored or reported as a construction error according to the library's requirement.

Registry values are available only on the unbuilt builder. They are cloneable configuration, not
access to a live handler, socket, or connection pool. Each type defines its own merge and
validation rules, and the selected transport consumes the final value during construction.

A companion configuration crate is introduced only for a demonstrated library need. If a setting
can be expressed with the same useful semantics across multiple transports, it may instead move to
a separately versioned semantic configuration crate. If its API necessarily names rustls,
native-tls, Hyper, or WinHTTP handles, it stays on the corresponding composition crate; splitting
such a type into another crate would not isolate its dependency.

## Transport contract

A transport configuration implements the complete portable contract and materializes the request
handler at the bottom of the pipeline. During client construction it receives:

- the resolved portable requirements;
- shared assembly services such as response-body infrastructure and telemetry;
- the runtime and threading context selected by its adapter.

Materialization remains fallible because a particular duration, credential binding, operating
system version, or external resource may be invalid or unavailable. These are value, provisioning,
or environment failures rather than structural capability mismatches, and they are reported before
requests are sent.

Runtime-selected and externally supplied transports use the same erased path. An external
transport is accepted only by implementing the full portable contract; partial transports do not
implement `Transport`.

## Transport composition and crate boundaries

`fetch_hyper_common` is the reusable TLS-neutral HTTP engine. It owns Hyper HTTP/1.1 and HTTP/2
dispatch, pooling, connection policy, bodies, errors, and telemetry. It accepts a `Connect` service
that already produces a usable cleartext or TLS stream.

TLS composition lives in accurately scoped crates:

```text
fetch_hyper_rustls     -> fetch_hyper_common + hyper-rustls + rustls
fetch_hyper_native_tls -> fetch_hyper_common + hyper-tls + native-tls
```

Each composition crate retains an application/runtime-provided network connector and backend
configuration in an unbuilt type implementing `fetch::Transport`. When `HttpClientBuilder::build`
supplies the final portable requirements, the composition crate configures TLS, SNI and ALPN, then
delegates handler construction to `fetch_hyper_common`. It does not duplicate the HTTP engine.
Backend-specific verifier, signer, identity, and provider types live with that composition crate.

WinHTTP is an independent full-stack transport. `fetch_winhttp` owns its sessions, pool, SChannel
integration, and asynchronous callback bridge; it does not use `fetch_hyper_common`.

Runtime integration supplies raw connectors and execution services. `fetch_m365`, for example,
adds Oxidizer runtime integration without creating another HTTP client or TLS API.

| Crate | Responsibility |
| --- | --- |
| `fetch` | Stable client, pipeline, portable requirements, transport construction contract, and typed config registry |
| `fetch_hyper_common` | Reusable TLS-neutral Hyper engine |
| `fetch_hyper_rustls` | Rustls connector composition and rustls-specific mechanisms |
| `fetch_hyper_native_tls` | Native-TLS connector composition and native-tls-specific mechanisms |
| `fetch_winhttp` | Independent WinHTTP transport and SChannel integration |
| `fetch_m365` | Oxidizer runtime connectors and execution services |
| `*_config` companion | Introduced only when a library demonstrably needs dependency-light configuration after erasure |

## Stability boundaries

Stabilizing `fetch` commits to `HttpClient`, `HttpClientBuilder`, the `Transport` construction
contract, portable requirement semantics, and the generic typed configuration-registry protocol.
It does not stabilize or re-export Hyper, WinHTTP, rustls, native-tls, or their configuration.

The Hyper engine, TLS composition crates, WinHTTP transport, and any dependency-light configuration
companions publish and evolve independently. A library opts into their stability and dependency
surface only by depending on them directly. Configuration that later proves useful across multiple
transports can move into a separate semantic crate without adding transport types to `fetch`.

Moving a portable requirement type into another crate does not remove its semantic commitment from
`fetch` when the stable builder accepts it. Separate crates isolate optional configuration and
dependencies; they do not disguise baseline behavior as unstable.

## Libraries configure outcomes, not mechanisms

Portable APIs describe guarantees with enough precision to validate them. They do not expose a
lowest-common-denominator options bag.

For example, connection maximum lifetime means that a connection is not selected for a new request
after the configured age. Hyper can enforce that contract by retiring or poisoning pooled
connections. WinHTTP can enforce it with `WINHTTP_OPTION_EXPIRE_CONNECTION`. The mechanism differs;
the guarantee does not.

When no faithful common contract exists, the option remains transport-specific. Coarse or partial
support is not silently treated as success. A library that intentionally requires the mechanism
uses a registered companion configuration type and reports an unsupported-transport error when it
is absent. Backend-typed configuration requires an explicit dependency on the composition crate.

## Protocol selection

Portable protocol configuration constrains the common HTTP/1.1 and HTTP/2 baseline. Its default is
no caller-imposed constraint; each composition supplies its normal protocol set. An explicit
portable requirement always takes precedence over a transport preference.

WinHTTP may expose `prefer_http3` on its composition builder. With no portable constraint, that
allows WinHTTP to try HTTP/3 and fall back to HTTP/2 or HTTP/1.1. An exact HTTP/2 requirement removes
HTTP/3 from consideration rather than conflicting with the preference. There is no transport-
specific `require_http3`; HTTP/3 becomes a portable requirement only when every supported transport
can implement it.

## Transport-owned performance policy

The stable API exposes service requirements, not copies of socket and protocol-stack knobs.
Supported transports choose and validate defaults for HTTP flow control, kernel buffering, and
congestion behavior.

Avoiding Nagle/delayed-ACK stalls is a transport invariant, not a configurable tuning choice. A
transport that owns TCP sockets disables Nagle. An opaque platform transport must demonstrate
equivalent small-write behavior in an integration benchmark; the WinHTTP probe does so for the
tested HTTP/1.1 path.

HTTP/2 receive windows remain transport-owned. Their useful value depends on bandwidth-delay
product, concurrent streams, response consumption, memory budget, and whether the implementation
uses adaptive flow control. Kernel send and receive buffers remain under operating-system
autotuning. Initial congestion behavior remains operating-system policy. None is configurable
through `HttpClientBuilder`.

## Body and content semantics

Request and response bodies are fallible streams of data and terminal trailers. Request APIs can
attach an asynchronously produced `Result` of trailers and declare that possibility before any
network I/O. A transport that cannot send request trailers rejects such a request before polling
or transmitting its body; it never discovers the mismatch after partial disclosure. Response
trailers are surfaced as a terminal fallible body frame.

Request trailers are intentionally not part of the universal transport baseline. Their
representation and failure semantics are stable in `fetch`, but execution remains fallible on a
transport such as WinHTTP whose native API cannot send them.

HTTP/2 transports support full-duplex streaming: response headers and body data may arrive before
the request body completes, and upload may continue afterward. An upload or trailer failure before
response headers fails request execution. A later upload failure remains observable through the
response lifecycle rather than being discarded. Dropping either side cancels the shared request
according to the normal cancellation contract.

Response decompression is an invariant `fetch` layer immediately above every transport, including
minimal and custom pipelines. Transports return wire-encoded bodies and do not enable native
automatic decompression. `fetch` advertises only encodings it can decode, streams decompression,
and normalizes the corresponding response headers uniformly across transports.

## TLS and credentials

TLS backend selection and backend-native customization are transport-specific. Portable security
requirements remain on `HttpClientBuilder` because libraries may own them.

Client-certificate authentication uses a logical credential identifier. For example, a TVS
library requests `ClientCredentialId::new("tvs-client")`. A Windows application may bind that name
to a certificate-store selector, while a Linux application binds the same name to provisioned
certificate and private-key material. The library selects the role but does not observe how the
application provides it.

A named binding may represent a set of rotating certificates. Rustls and WinHTTP can select a
certificate using issuer hints received during the handshake. The current native-TLS adapter must
resolve the binding to one identity when constructing the connector and therefore requires a client
rebuild to pick up rotation.

Portable server validation is split into platform chain trust and endpoint identity. Platform
trust, hostname validation, and revocation are baseline security behavior. For example, a request
may remain addressed to `https://localhost:50042`, so the server sees `localhost:50042`, while the
builder maps that origin to the exact TLS name `tvs.prod.example`. The transport still connects to
localhost but sends `tvs.prod.example` as SNI and validates that DNS name against the certificate.
All supported transports can provide this contract without a custom validation callback.

Arbitrary SAN patterns, subject distinguished-name allowlists, certificate/public-key pins, and
per-client custom trust roots are not portable capabilities and are explicit non-goals. WinHTTP
and the supported native-TLS path cannot safely enforce them before request headers or credentials
may be disclosed. A library that truly requires one must depend on a supporting composition crate
and reject other transports. If TVS cannot use a stable exact DNS identity present in its
certificates, TVS cannot remain transport-independent under this design.

A raw rustls verifier callback remains a rustls-specific mechanism.

## Requirement composition

Portable settings are accumulated as constraints rather than applied with unrestricted
last-write-wins semantics.

- compatible bounds merge to the stricter result;
- an application may add or tighten a library requirement but may not weaken it;
- equivalent credential requirements are deduplicated;
- incompatible required credentials or policies fail construction with their sources identified.

An explicitly configured portable option is required by default. The initial stable surface does
not silently downgrade performance requirements. Preferences may be added only with a resolution
report that lets the caller observe whether and how they were applied.

## Telemetry

The pipeline creates one telemetry scope for the client. The selected transport receives the
corresponding meter during materialization and records transport events within that scope. Runtime
and transport names are stable attributes supplied by their adapters.

A transport does not require callers to provide a second telemetry sink or meter.

## Dependency and feature selection

`fetch` does not select a runtime, Hyper, WinHTTP, TLS backend, or crypto provider through features.
Applications select a transport by depending on a composition crate and constructing it at the
composition root. Libraries do not enable a concrete backend merely to express portable
requirements.

TLS features and provider dependencies remain inside their composition crates. An application
using WinHTTP does not acquire Hyper or rustls; one using Hyper with native TLS does not acquire
rustls through feature unification. A rustls-specific verifier necessarily requires
`fetch_hyper_rustls`, because hiding that real dependency behind a nominally lightweight config
crate would not improve governance or stability.

## Public API boundary

`HttpClientBuilder` contains pipeline policy and portable transport requirements. Its methods are
available regardless of the selected supported transport. The builder does not contain
transport-specific configuration or backend capability branches.

Composition builders contain backend selection and native tuning. The transport interface receives
resolved portable requirements and registered dependency-light configuration rather than a
Hyper-shaped options structure.

Building returns a concrete `HttpClient` and may fail for invalid values, unresolved named
credentials, unavailable runtime resources, or an unsupported host version. Unsupported security
or networking configuration is never ignored, approximated without an explicit contract, or
discovered only after a request begins.
