# Transport configuration

This document defines how libraries configure networking requirements without choosing or
understanding the selected transport.

## Portable requirements

A portable requirement describes an outcome that can be implemented by different mechanisms. Its
contract is precise enough for a transport to decide whether a particular value is supported.

Representative requirements include:

- maximum connection age before retirement;
- maximum idle age before a connection is no longer reused;
- connection limits per destination;
- connect deadline;
- required and preferred HTTP protocol versions;
- client-certificate authentication;
- exact TLS server-name mapping;
- server-certificate trust and pinning policy, if a portable contract is later required;
- cancellation and streaming guarantees.

These requirements are stored separately from pipeline configuration and from the concrete
transport configuration.

```rust,ignore
pub struct TransportRequirements {
    connection: ConnectionRequirements,
    protocols: ProtocolRequirements,
    security: SecurityRequirements,
}
```

The public types describe semantics, not backend controls. For example, a maximum connection
lifetime is an upper bound on reuse, not a Hyper pool-poisoning interval or a WinHTTP session
timeout.

## Builder and transport mechanics

`HttpClientBuilder` owns an erased transport configuration and an accumulated set of portable
requirements. Its setters merge constraints and retain enough provenance to diagnose conflicts.
`build` returns the concrete, transport-erased `HttpClient`.

`Transport` defines the complete library-facing baseline:

```rust,ignore
pub trait Transport: Send + Sync + 'static {
    fn build(
        self: Box<Self>,
        requirements: TransportRequirements,
        context: TransportContext,
    ) -> Result<TransportHandler, TransportBuildError>;
}
```

The application passes any complete transport to the same constructor:

```rust,ignore
let builder = HttpClient::builder(transport);
```

`build` performs value and environment validation because structural support does not imply that
every value is valid. For example, a transport can support connection lifetime while rejecting an
out-of-range duration, or require a named client credential that the application did not bind.

The transport receives no generic TLS or connection-options bag. Each implementation translates
the semantic requirements directly into its own configuration.

## Composition and erasure

Transport erasure occurs when the application creates the builder. Libraries then configure one
stable type without understanding the underlying transport:

```rust,ignore
pub fn configure(builder: HttpClientBuilder) -> Result<HttpClient> {
    builder
        .connection_lifetime(LIFETIME)
        .client_certificate(ClientCredentialId::new("service-client"))
        .tls_server_name(
            Origin::https("localhost", SERVICE_PORT),
            ServerName::new("tvs.prod.example")?,
        )
        .build()
}
```

The resulting client is likewise non-generic. Runtime-selected transports, application-selected
transports, and fakes all follow this path. A fake implements the full transport contract and can
record the received requirements.

This design deliberately rejects partial transport capability profiles. A transport that cannot
implement a library-facing baseline requirement is not a `fetch` transport. Differences in
supported values or operating-system availability remain construction-time validation because
Rust types cannot prove those environmental facts.

### Typed transport extensions

Erasure hides the transport from the portable API but does not make intentional backend integration
impossible. Until `build`, the builder retains cloneable, type-indexed extension values registered
by the selected transport:

```rust,ignore
if let Some(options) = builder.transport_extension_mut::<WinHttpOptions>() {
    options.use_integrated_proxy_discovery(true);
}
```

Querying an extension is the supported way to identify a transport capability outside the portable
baseline. A required extension is checked at runtime because the builder is concrete and the
application may select its transport dynamically. Libraries using this path depend on the concrete
transport crate and must define what absence means.

Extensions contain unbuilt configuration only. They cannot expose live sockets or handlers, and
they disappear when the builder is consumed. The type's own API defines whether contributions
merge, replace, or conflict; transport construction validates the result. This keeps unchecked
`Any` downcasts and string transport identifiers out of library code while preserving the simple
non-generic builder.

## Library-facing surface

The demonstrated library requirements fit one coherent builder:

| Concern | Portable contract |
| --- | --- |
| Connection lifetime | Do not select a connection for a new request after its maximum age |
| Idle lifetime | Do not reuse a connection after the configured idle age |
| Connection limit | Bound total concurrent connections per origin |
| Connect deadline | Bound establishment of a usable connection |
| HTTP versions | Express ordered preferences and strict protocol requirements |
| Client authentication | Select a logical credential role provisioned by the application |
| TLS endpoint identity | Authenticate an exact DNS name for a scoped request origin |
| Pipeline behavior | Compose routing, resilience, telemetry, redaction, and response policy |

Streaming, cancellation, standard chain trust, hostname validation, and revocation are transport
invariants rather than optional builder settings. Backend tuning, proxy discovery, integrated
authentication, custom verifier callbacks, and credential source modalities stay on concrete
transport builders because applications own those mechanisms.

Routine transport tuning is narrower still: a mechanism is not exposed merely because a backend
offers a setter. HTTP flow-control sizing, kernel socket buffers, and congestion startup remain
implementation policy unless a measured workload establishes a stable outcome that callers need
to control.

## Requirement strength

Explicit portable configuration is a requirement unless the API says otherwise. This keeps a
library's correctness, security, and resource assumptions from becoming best-effort behavior when
an application selects a different transport.

Requirement types encode the guarantee:

- `ConnectionLifetime::at_most(duration)` limits reuse by connection age;
- `ConnectionIdleAge::at_most(duration)` limits reuse after inactivity;
- a protocol requirement distinguishes an ordered preference from a strict minimum or prohibition;
- security policies are always required.

An implementation either establishes the guarantee or returns an error. An implementation with
coarser behavior can satisfy a requirement only when the coarse behavior still implies the stated
guarantee. For example, retiring a connection earlier than a configured maximum lifetime is valid;
retiring it later is not.

Portable preferences are a separate concept. If introduced, construction returns a resolution
report containing every unmet or coarsened preference. A required option never degrades through the
preference mechanism.

## Constraint composition

Multiple callers may contribute requirements to one builder. Setters merge constraints instead of
overwriting prior values.

Monotonic constraints combine naturally:

- maximum ages and connection counts take the lowest bound;
- minimum protocol or security constraints take the strongest compatible bound;
- allowed sets intersect;
- preferred orderings combine only when they do not violate requirements.

Singleton resources require agreement. Two equivalent client-certificate sources are one
requirement; two distinct required sources conflict unless the policy explicitly scopes selection.
Errors identify the conflicting requirements and which configuration layer supplied them.

There is no unrestricted "application wins" or "library wins" precedence. A later caller can
tighten a requirement. Weakening or replacing it requires an explicit API that proves the earlier
owner allowed replacement.

## Client-certificate authentication

The builder names one client credential per applicable destination scope:

```rust,ignore
builder.client_certificate(ClientCredentialId::new("service-client"))
```

`ClientCredentialId` is a stable logical role, not a thumbprint, subject name, file path, or store
location. Those values identify a particular provisioning mechanism or certificate generation and
would force library code to understand deployment details. A logical identifier remains stable
through certificate rotation and across operating systems.

The application supplies a certificate catalog when constructing the transport and binds logical
identifiers to transport-native sources:

```rust,ignore
let transport = fetch::transport::hyper(runtime)
    .rustls(rustls)
    .client_certificates(
        ClientCertificateCatalog::new()
            .bind_windows_store("service-client", service_selector),
    );
```

Concrete transport builders expose the binding forms they can consume. Rustls can bind key material,
a Windows-store selector, or a signing provider. Native TLS can bind a materialized platform
identity. WinHTTP can bind a Windows-store selector or imported key material. These forms are not
part of the portable `HttpClientBuilder` API.

Every supported transport implements named client-certificate authentication, so source modality
does not require a capability trait. A missing identifier or a binding that cannot be materialized
is a construction-time provisioning error, analogous to a missing named credential.

Catalog entries may represent one certificate or an ordered set used for rotation. Rustls receives
signature schemes and acceptable issuer distinguished names during its handshake and can select a
compatible entry without exposing an identity unnecessarily. WinHTTP reports that a client
certificate is needed and exposes the server issuer list before the request is retried, enabling
the same selection. The current native-TLS API accepts one identity on the connector, so its
catalog must select during construction and rotation requires rebuilding the client.

Certificate discovery is fallible and can expose sensitive metadata. Transport builders may list
registered logical identifiers and sanitized public-certificate descriptors for diagnostics.
Libraries select a known logical identifier rather than enumerating certificates and inventing
selection policy.

Two different required identifiers for the same destination conflict. Rebinding an identifier is
an application composition operation and is not available to a library after the transport builder
has been handed off.

## Connection lifetime

Connection maximum lifetime is portable because the observable contract is portable even though
pool implementations differ.

Hyper records connection age and prevents an over-age connection from serving a new request.
WinHTTP marks the connection serving a request for retirement with
`WINHTTP_OPTION_EXPIRE_CONNECTION` when its age reaches the configured bound. Both satisfy the
same upper-bound contract.

Idle-age policy is also expressed as a bound, but supported values differ. Hyper can enforce the
configured bound in its pool. WinHTTP can shorten its native idle behavior and can retain HTTP/2
connections with keep-alive PINGs, but cannot guarantee every longer HTTP/1.1 retention request.
The WinHTTP transport accepts values for which it can prove the portable contract and rejects the
rest.

Fine-grained HTTP/2 PING interval, acknowledgement timeout, and pool-poisoning settings remain
Hyper-specific because they configure mechanisms rather than portable outcomes.

## Data-path tuning policy

The initial API does not expose the inherited socket and HTTP/2 tuning knobs.

| Mechanism | Policy |
| --- | --- |
| Nagle algorithm | No caller setting. Socket-owning transports enable `TCP_NODELAY`; opaque transports must demonstrate equivalent small-write behavior. |
| HTTP/2 initial stream receive window | Transport-selected policy. Prefer a mature adaptive strategy where available; otherwise choose a validated fixed default. |
| Socket receive and send buffers | Leave to operating-system defaults and autotuning. |
| Initial TCP congestion window | Leave to the operating system and network policy. |

The Nagle decision is an invariant because ACK-dependent delays harm request headers, small
streaming bodies, HTTP/2 control frames, and multiplexed RPC traffic. Protocol-aware write
coalescing remains desirable, but it occurs before TCP and does not replace `TCP_NODELAY`.
WinHTTP does not expose the socket setting, so its conformance is behavioral: a calibrated probe
shows its HTTP/1.1 upload path tracking a `TCP_NODELAY` control rather than a Nagle control. This is
retained as regression evidence, not treated as a documented WinHTTP guarantee.

An HTTP/2 stream window is a receiver memory-and-throughput policy, not a service guarantee. A
small window can make a high-bandwidth, high-latency response RTT-bound; a large window grants more
outstanding data for every active stream. Hyper also offers adaptive flow control, while WinHTTP's
window-update strategy and default are OS-owned. A portable numeric setter would expose only one
piece of those policies and invite libraries to impose memory costs without knowing application
concurrency.

`SO_RCVBUF` and `SO_SNDBUF` are kernel queue capacities, distinct from application buffers, TLS
records, TCP receive-window autotuning, and HTTP/2 flow control. Fixed values can constrain
autotuning and multiply memory consumption by connection count. Application-level buffering may
still be tuned internally to reduce I/O operation and allocation overhead.

Initial congestion-window selection affects only connection startup, is path-dependent, and can
increase burst loss or unfairness. Pooling and HTTP/2 amortize its effect. The Windows per-socket
control is nonportable and poorly documented, and WinHTTP exposes no equivalent.

These defaults require representative benchmarks rather than permanent configurability. A future
option needs evidence that the default causes a material problem, a precise observable contract,
and a coherent ownership model. Until then it is neither a portable builder method nor a supported
advanced transport option.

## TLS policy

TLS backend selection belongs to the concrete Hyper transport builder. WinHTTP always uses
SChannel.

Portable security policy is configured through semantic requirements:

- logical client-credential identifier;
- exact TLS server name for a request origin;
- trust anchors or platform trust;
- minimum TLS properties;
- mandatory revocation behavior.

Each transport either enforces the policy or rejects construction. Security policy is never
approximated.

Backend-native extension points stay on concrete builders. A raw rustls verifier, prebuilt rustls
configuration, native-TLS connector, or SChannel option is intentionally unavailable through the
portable builder.

### Endpoint and TLS identity

Requests ordinarily use one origin for three related purposes:

- the network destination (`D`);
- the DNS name authenticated by TLS (`L`);
- the HTTP `Host` or `:authority` value (`H`).

The baseline permits a library to replace only `L` for a scoped HTTPS origin:

```rust,ignore
builder.tls_server_name(
    Origin::https("localhost", service_port),
    ServerName::new("tvs.prod.example")?,
)
```

The request remains addressed to `https://localhost:<port>`. The transport connects to the
original host and port, authenticates `tvs.prod.example`, and sends `localhost:<port>` as the HTTP
authority. Ports are part of the origin, routing, authority, and pool key, but not SNI or
certificate DNS-name matching. The API therefore scopes a mapping by the complete origin while
accepting only a DNS name as the replacement identity.

Mappings are exact and fixed at client construction. They do not accept verifier callbacks,
regular expressions, certificate subjects, or alternate ports. This keeps transport mechanisms
out of library code and avoids exposing modalities that the demonstrated localhost-to-service
scenario does not need.

Each backend lowers the same contract at its transport boundary:

- Hyper with rustls dials `D`, supplies `L` to rustls, and preserves `H` on the request;
- Hyper with native TLS dials `D`, calls the native TLS handshake with `L`, and preserves `H`;
- WinHTTP passes `L` to `WinHttpConnect`, sets `D` through
  `WINHTTP_OPTION_RESOLUTION_HOSTNAME`, and replaces `Host` with `H`.

WinHTTP documents the resolution override and generic `Host` replacement. Current Windows
versions also translate the replacement `Host` into HTTP/2 `:authority`, as verified by the
executable backend probe, but Microsoft does not explicitly document that translation. The
backend retains an integration test and fails construction on Windows versions that lack the
resolution option. The complete documentation audit and executable evidence are recorded in the
[WinHTTP resolution-hostname experiment](../../../fetch_winhttp/docs/resolution-hostname-experiment.md).

Authority replacement is transport-generated state, not a persistent user header. A redirected or
retried request recomputes `D`, `L`, and `H` from its effective origin and mapping; it must not
blindly carry an earlier origin's replacement `Host` value to another destination.

Connection reuse must be partitioned by the effective tuple `(D, port, L, H)`. Two origins or TLS
identity mappings must never share a connection merely because WinHTTP or a Hyper pool would
otherwise consider their default authority equal.

This exact-name contract intentionally does not preserve the current TVS validator's open-ended
SAN regular expressions or subject-name allowlists. Those rules can be replaced only when the
service supplies a concrete DNS identity present in its certificates. Flexible matching, pinning,
and custom roots cannot be portable requirements because not every supported transport can enforce
them before disclosing a request. A transport-bound library may configure such a policy through a
typed extension and must reject transports that do not expose it.

## Growing the portable surface

The backend inventory and decisions about which differences belong on the public builder are
maintained in the [capability matrix](capability-matrix.md).

The initial surface has no capability traits. A new library-facing requirement is added to the
portable contract only when it has precise observable semantics and every supported transport can
implement it. Otherwise it remains transport-specific configuration, reachable by libraries only
through typed extensions. If a future requirement is essential to transport-independent libraries
but fundamentally unavailable on a supported transport, the supported transport set or this design
must change; a marker trait cannot manufacture the missing behavior.
