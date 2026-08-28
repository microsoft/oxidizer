# Transport capability matrix

This document compares the supported Hyper TLS combinations with the planned WinHTTP transport. It
separates differences that matter to libraries from backend mechanisms that should not expand the
portable `fetch` API.

The WinHTTP column describes the design on `u/makolnek/winhttp`; implementation work must verify
the stated guarantees.

The two Hyper columns share one TLS-neutral `fetch_hyper` engine. `fetch_hyper_rustls` and
`fetch_hyper_native_tls` provide connector composition, not separate HTTP implementations.

## TLS and client authentication

| Capability | Hyper + rustls | Hyper + native TLS | WinHTTP + SChannel | Public treatment |
| --- | --- | --- | --- | --- |
| Platform trust | Platform verifier | Native platform trust | Native Windows trust | Baseline/invariant |
| Named client credential | Catalog can bind key material or a signing resolver | Catalog binds a materialized native identity | Catalog can bind a store selector or imported material | Baseline |
| Exportable certificate and private key binding | Supports common key encodings | Requires PKCS#8 through the current adapter | Planned import into a temporary certificate store | Transport construction |
| Non-exportable Windows-store binding | Supported through a rustls signing resolver | No equivalent current API | Supported through `CERT_CONTEXT` | Transport construction |
| Arbitrary external signing service | Supported through rustls signing traits | Unsupported | Unsupported unless it provides a compatible Windows key handle | `fetch_hyper_rustls` composition only |
| Custom verifier callback | Supported | Unsupported | No userspace callback | `fetch_hyper_rustls` composition only |
| Exact TLS server-name override while preserving request authority | Connector dials the request endpoint and supplies the override to rustls | Connector dials the request endpoint and supplies the override to native TLS | `WinHttpConnect` uses the TLS name, resolution override uses the endpoint, and replaced `Host` preserves authority | Baseline |
| Custom SAN/subject server-identity policy | Enforced before application data by a verifier | Unsupported | Cannot be safely enforced before request disclosure | Explicit portable non-goal; `fetch_hyper_rustls` only |
| Certificate or public-key pins | Implementable in a verifier | Unsupported | Cannot be safely enforced before request disclosure | Explicit portable non-goal; supporting composition crate only |
| Per-client custom trust roots | Expressible through custom rustls configuration | Not exposed by the current adapter | Uses Windows trust stores | Explicit portable non-goal; supporting composition crate only |
| TLS backend and crypto-provider selection | `fetch_hyper_rustls` | `fetch_hyper_native_tls` | SChannel is fixed | Application selects a composition dependency |
| Revocation | Required by the platform-verifier policy | Platform behavior | Must be enabled explicitly | Invariant, not a capability |

Libraries select a stable logical client-credential identifier. Applications bind that identifier
to key material, a Windows-store selector, or another transport-native provider. This makes source
modality a transport-construction concern rather than a library-facing capability axis.

## HTTP protocols

| Capability | Hyper + either TLS backend | WinHTTP | Public treatment |
| --- | --- | --- | --- |
| HTTP/1.1 and HTTP/2 preference | Supported | Supported | Baseline |
| Strictly require HTTP/2 | Supported by Hyper's HTTP/2-only mode | Supported by enabling HTTP/2 and setting `WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED` | Baseline; gRPC is a demonstrated consumer |
| Initial HTTP/2 stream receive window | Fixed or adaptive policy | OS default; a fixed window option exists | Transport-owned default, not public configuration |
| HTTP/3 | Unsupported | Supported on recent Windows | Transport-specific until Hyper and every supported transport implement it and a library requires it |
| Fine-grained HTTP/2 flow control | Supported | Different partial native controls | Internal transport policy |

An ordered version preference and a protocol requirement are different APIs. A transport may honor
an HTTP/2 preference by falling back to HTTP/1.1, but that is not sufficient for a gRPC library
that requires HTTP/2. WinHTTP can prevent fallback by combining its HTTP/2 enable flag with
`WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED`; no user-space emulation is needed. The option requires
Windows 10 version 1903 or later. On an older supported host, transport construction for a strict
HTTP/2 requirement fails.

The receive window is not a protocol requirement. Its optimum depends on path bandwidth and RTT,
active stream count, response consumption, memory budget, and adaptive-window behavior. A numeric
cross-transport option would expose only part of that policy. Transports select and benchmark their
own defaults.

## Connections

| Capability | Hyper + either TLS backend | WinHTTP | Public treatment |
| --- | --- | --- | --- |
| Fixed maximum connection lifetime | Enforced by retiring aged connections | Planned through `WINHTTP_OPTION_EXPIRE_CONNECTION` | Baseline |
| Per-connection lifetime callback | Supported by current Hyper options | No portable callback model | Transport-specific; fixed lifetime covers the demonstrated library need |
| Maximum idle age | Arbitrary duration or unlimited | Shortening is supported; longer retention depends on protocol and native scavenging | Baseline with value/protocol validation |
| Total connections per origin | Not provided by the current Hyper option | Native per-server cap | Baseline requirement, but Hyper needs a real total-concurrency implementation |
| Maximum idle connections per host | Current Hyper `max_connections` behavior | No equivalent meaning; WinHTTP's cap is total connections | Rename and keep transport-specific |
| Coarse idle HTTP/2 health check | Supported | Supported with a native minimum interval | Transport-owned policy; no public option |
| Keep-alive interval, acknowledgement timeout, and active-only mode | Supported by Hyper | Not available at the same granularity | Transport-specific |
| Multiple dispatch pools | Implemented above the transport | Can use multiple transport instances | Pipeline policy, not a transport capability |
| Avoid Nagle/delayed-ACK stalls | Set `TCP_NODELAY` on owned sockets | No setter, but calibrated HTTP/1.1 measurements match `TCP_NODELAY` behavior | Transport invariant with backend regression coverage |
| Kernel receive/send buffers | Configurable on owned sockets | No corresponding WinHTTP option | Operating-system default/autotuning |
| Initial TCP congestion window | Windows custom connectors can call an undocumented `WSAIoctl`; no portable equivalent | No corresponding WinHTTP option | Operating-system/network policy |

The current `ConnectionPoolOptions::max_connections` name is misleading: Hyper forwards it to
`pool_max_idle_per_host`, while downstream TVS configuration describes a maximum number of
concurrent connections per server. These are different guarantees and must become different
options rather than backend mappings of one field.

Connection idle age is value-dependent rather than a useful type-level distinction. WinHTTP can
honor an upper bound by closing earlier, but cannot promise arbitrary long HTTP/1.1 retention.
Construction validates the requested value together with the protocol requirement.

Nagle control is deliberately not configurable. General-purpose HTTP needs prompt semantic
boundaries and control frames; bulk transfers already fill segments, while protocol-aware
coalescing provides efficiency without ACK-dependent delay. The
[WinHTTP experiment](../../../fetch_winhttp/docs/nagle-behavior-experiment.md) verifies equivalent
behavior on the tested path but does not turn an undocumented implementation detail into a
platform guarantee.

Fixed socket buffers and initial congestion windows are not portable outcomes. They can defeat
kernel adaptation, consume memory per connection, or tune one network at the expense of another.
Application buffers remain separate implementation details.

## Timeouts, I/O, and platform services

| Capability | Hyper + either TLS backend | WinHTTP | Public treatment |
| --- | --- | --- | --- |
| End-to-end and attempt deadlines | Pipeline-owned | Pipeline-owned | Pipeline policy |
| Connect deadline | Wraps connector establishment | Native resolve/connect controls with different phase boundaries | Baseline after defining one observable deadline |
| Separate resolve/send/receive timers | Not exposed by the supported Hyper path | Native controls | Transport-specific |
| Streaming request and response bodies | Supported | Planned | Required `Transport` invariant |
| Cancellation when the request future is dropped | Supported | Planned through handle closure | Required `Transport` invariant |
| Plain HTTP opt-in | Pipeline request validation plus transport support | Supported | Pipeline policy |
| Runtime/executor selection | Hyper requires an adapter | WinHTTP owns asynchronous I/O callbacks | Transport construction |
| Proxy discovery and integrated Windows authentication | Not in the standard Hyper connector | Native WinHTTP strengths | Transport-specific application configuration |

## Demonstrated library requirements

Current downstream code demonstrates a smaller set than the theoretical backend inventory:

- gRPC requires HTTP/2 and configures a connect timeout;
- the TVS client configures exportable client key material, a server-certificate validator,
  connection limits, idle age, and detailed HTTP/2 keep-alive values;
- fetch integration tests exercise fixed maximum connection lifetime.

These uses do not automatically justify preserving every existing knob:

- the TVS connection-limit setting currently maps to Hyper's idle-pool limit rather than the
  configured concurrent-connection meaning and needs correction;
- detailed keep-alive timings are Hyper mechanisms unless the service can state a portable outcome
  it requires;
- inherited C# settings for HTTP/2 windows, socket buffers, and initial congestion do not by
  themselves demonstrate a library requirement; transport defaults remain until representative
  benchmarks show a material deficit;
- the rustls `Validator` should first be reduced to platform trust plus an exact TLS server name.
  Its SAN-pattern and subject-name modalities survive only if concrete TVS deployments cannot name
  one DNS identity present in their certificates.

## Public surface conclusion

The current evidence does not justify capability traits or a transport-generic
`HttpClientBuilder<T>`. Every demonstrated library requirement has a common observable contract,
and exact TLS server-name mapping covers the need to reach a local endpoint while authenticating a
service DNS name. The public builder can therefore be one concrete type with the same methods for
all supported transports.

Custom server identity remains outside the proposed surface. It would cover service-defined SAN
patterns or known subject names only if TVS cannot migrate to an exact DNS identity. Hyper/rustls
supports that richer mechanism; the current native-TLS adapter and WinHTTP do not. It therefore
cannot become a portable capability. A TVS library that retains it must select a supporting
transport through `fetch_hyper_rustls` and reject the others.

Named client credentials, exact TLS server-name mapping, strict HTTP/2, fixed connection lifetime,
connect deadline, connection limits, streaming, cancellation, and ordinary HTTP/1.1/HTTP/2
preferences belong to the baseline and do not need capability traits.

Arbitrary signers, raw verifier callbacks, and backend TLS objects stay on composition builders.
Detailed keep-alive controls, HTTP/3, proxy/WPAD, integrated authentication, phase-specific
timeouts, and pool internals stay composition-owned unless a demonstrated library need justifies a
dependency-light companion configuration crate. HTTP/2 flow-control sizing, socket buffers, and
initial congestion are transport-owned defaults rather than public options on any builder.

Construction remains fallible despite the uniform surface. Invalid values, missing named
credentials, unsupported operating-system versions, and unavailable resources are environmental
or provisioning failures; they are not evidence for a type-level transport capability.
