# `fetch` design

`fetch` provides a stable HTTP client and configuration surface over transports with different
capabilities. Applications choose a transport, while libraries can configure the portable
networking behavior they require without knowing which transport the application selected.

The crate may use focused implementation crates for Hyper, TLS, and platform transports. Those
package boundaries are hidden by the supported `fetch` API.

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
   Applications normally set these options on a concrete transport builder before passing it into
   `fetch`. A library that deliberately supports a particular backend may inspect and modify its
   registered typed transport extensions before construction. Examples include rustls verifier
   callbacks, WinHTTP proxy discovery, and SChannel-specific options. A backend mechanism does not
   automatically deserve a public transport option; routine flow-control, socket-buffer, and
   congestion tuning remains transport-owned.

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

### Typed transport extensions

Some libraries intentionally integrate with one or more specific transports. The erased builder
therefore carries a type-indexed set of transport extension values until `build`. A transport
registers its public extension types, and a library may query or mutate one by its Rust type:

```rust,ignore
let winhttp = builder
    .transport_extension_mut::<fetch_winhttp::WinHttpOptions>()
    .ok_or(BuildError::WinHttpRequired)?;

winhttp.use_integrated_proxy_discovery(true);
```

The presence of an extension identifies support; transport names and string comparisons are not
part of the contract. A library that supports several transports can branch over their extension
types. If a transport-specific setting is required, absence is a construction error chosen by that
library. Optional tuning may simply leave unmatched transports unchanged.

Extensions are available only on the unbuilt builder. They are cloneable configuration values, not
access to a live handler, socket, or connection pool. Each extension type defines its own mutation
and validation rules, and the transport validates the final value during construction. Using one
creates an intentional dependency on that transport crate and provides no guarantee for other
transports; it does not enlarge the portable `fetch` contract.

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

## Supported transports and runtimes

The supported Hyper transport combines a connector, runtime services, a TLS backend, and
transport-specific tuning. Tokio is a supported runtime adapter in `fetch`.

WinHTTP is a full-stack transport rather than a Hyper connector. It owns its sessions, connection
pool, SChannel integration, and asynchronous callback bridge. It participates in the same portable
requirements contract as Hyper while retaining its native configuration surface.

Other runtime crates may supply connectors and execution services to supported transports without
creating a separate HTTP client API.

## Libraries configure outcomes, not mechanisms

Portable APIs describe guarantees with enough precision to validate them. They do not expose a
lowest-common-denominator options bag.

For example, connection maximum lifetime means that a connection is not selected for a new request
after the configured age. Hyper can enforce that contract by retiring or poisoning pooled
connections. WinHTTP can enforce it with `WINHTTP_OPTION_EXPIRE_CONNECTION`. The mechanism differs;
the guarantee does not.

When no faithful common contract exists, the option remains transport-specific. Coarse or partial
support is not silently treated as success. A library that intentionally requires the mechanism
uses a typed transport extension and reports an unsupported-transport error when it is absent.

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
may be disclosed. A library that truly requires one must use a typed extension for a transport
that supports it and reject other transports. If TVS cannot use a stable exact DNS identity present
in its certificates, TVS cannot remain transport-independent under this design.

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

## Features

The core client and transport traits are available without selecting a runtime or TLS backend.
Features add supported runtime, transport, and TLS implementations.

Feature selection never resolves ambiguity by order. When multiple TLS backends are enabled, the
application selects one on the concrete transport builder or accepts a documented preset.
Libraries do not enable a concrete backend merely to express portable requirements.

## Public API boundary

`HttpClientBuilder` contains pipeline policy and portable transport requirements. Its methods are
available regardless of the selected supported transport. The builder does not contain
transport-specific configuration or backend capability branches.

Concrete transport builders contain backend selection and native tuning. The transport interface
receives resolved portable requirements rather than a Hyper-shaped options structure.

Building returns a concrete `HttpClient` and may fail for invalid values, unresolved named
credentials, unavailable runtime resources, or an unsupported host version. Unsupported security
or networking configuration is never ignored, approximated without an explicit contract, or
discovered only after a request begins.
