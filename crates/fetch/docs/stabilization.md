# `fetch` API stabilization feedback

The Windows-only `fetch_winhttp` transport exposed API assumptions that come from the original
Hyper and Tokio transport. None of these issues blocks `fetch_winhttp`. Resolve or accept them
before the `fetch` API becomes stable.

`fetch` assembles the HTTP request and response pipeline. A transport performs network I/O.
Transport configuration includes TLS, network-phase timeouts, connection lifetime, and connection
reuse. Pipeline configuration includes the total response deadline, retries, and telemetry.

The reviewer input below records options from the `fetch_winhttp` review. It does not record final
decisions.

## 1. First-class transport plug-in

`HttpClient` and its builder are in `fetch`. The orphan rule prevents a downstream crate from
adding inherent constructors. Therefore, `fetch_winhttp` uses an extension trait named
`HttpClientWinHttpExt` to provide `HttpClient::builder_winhttp`.

Built-in transports have inherent methods such as `HttpClient::builder_tokio` and
`HttpClient::new_tokio`. A downstream transport requires an imported trait. The call syntax hides
some of this difference but does not remove it.

One option is a transport-neutral constructor. The caller constructs and passes the transport:

```rust,ignore
HttpClient::build(fetch_winhttp::transport().tls(cfg))
```

This design treats each transport the same and does not make Hyper the implicit default.

Reviewer input also supports a built-in default. Most applications use the recommended transport
for their runtime. They can keep a short constructor while custom transports remain available.
One proposed type-state design is:

- `HttpClient::builder()` returns `HttpClientBuilder<NeedsTransport>`.
- `HttpClientBuilder<NeedsTransport>` exposes only transport-neutral pipeline configuration. It
  cannot build until the caller adds a transport.
- `HttpClient::builder_tokio(...)` returns `HttpClientBuilder<DefaultTransport>`.
- `HttpClientBuilder<T = DefaultTransport>` keeps the generic parameter off `HttpClient`.
- A method such as `hyper(hyper_transport)` exposes deeper configuration of the default transport.

The current transport bundling can remain temporary until runtime and security design is stable.
After that work, built-in `builder_oxidizer` and `new_oxidizer` constructors can provide the
recommended transport.

The current transport list can also become closed. `fetch` can bundle `fetch_winhttp` after the
out-of-tree transport moves into this repository.

Another option follows the .NET package split for
`System.Net.Http.WinHttpHandler`. A `fetch_core` crate can contain the pipeline without transports.
The `fetch` crate can re-export `fetch_core` and supported transports. Libraries can depend on
`fetch_core`. Applications can depend on `fetch`.

## 2. Transport-specific TLS configuration

`TlsOptions` contains rustls and native-tls material that `fetch_hyper` can use. The Schannel-based
`fetch_winhttp` transport cannot use that material. See `fetch_winhttp` `design.md` sections 1.2
and 4.

TLS belongs to the transport because each backend exposes different capabilities. For example,
rustls supports certificate-validation callbacks. native-tls and Schannel do not expose the same
callback.

One option moves all TLS configuration to the transport:

```rust,ignore
HttpClient::build(transport().tls(cfg))
```

Another option keeps transport-neutral TLS descriptors in `fetch`. Each transport interprets only
the capabilities that it supports. A related constructor can require TLS explicitly:

```rust,ignore
HttpClient::builder_tokio(deps, tls)
```

The current shape can silently ignore security configuration. `CustomContext::tls` is always
present. A caller can set a private CA root, client identity, or certificate pinning, then use a
transport that cannot apply it. `fetch_winhttp` can then use default system trust without a signal.

Do not fix this by adding field-by-field warnings in every transport. Put TLS configuration on the
transport that can apply it. Unsupported combinations then become unrepresentable.

## 3. Transport-specific connection management

`fetch` exposes `max_connections`, `connection_idle_timeout`, `connection_lifetime`, and
`ConnectionKeepAlive`. See `fetch_winhttp` `design.md` section 2.1.

Each transport manages connections differently:

- `max_connections` assumes that `fetch` owns the pool. WinHTTP owns its pool and has different
  limits.
- WinHTTP has no per-connection age control, so it cannot implement `connection_lifetime`. See
  `fetch_winhttp` `design.md` section 2.2.
- WinHTTP applies protocol-specific keep-alive values with its own minimums.
  `ConnectionKeepAlive` and `connection_idle_timeout` are approximate or unsupported mappings.

These settings belong on the transport.

The pool model also needs a decision. `fetch` exposes `multiple_pools` and `PoolIndex`. It calls a
custom transport factory once for each pool slot. WinHTTP ignores the `PoolIndex` value, but each
factory call creates a separate WinHTTP session and pool. The resource model is one session and
pool for each thread and pool slot. It is not one shared pool.

If transports own connection management, they probably also own pool partitioning. That design can
remove `PoolIndex` in its current form. Decide this in the version 2 sessions and pools discussion.

## 4. Timeout configuration by scope

`fetch` exposes a connect timeout but no resolve or send timeout. The missing timers are not the
main problem. Each transport supports different timers and phase boundaries.

Resolve, connect, send, and receive timeouts belong to the transport. The total request deadline
belongs to the pipeline. See `fetch_winhttp` `design.md` section 6.1.

Reviewer input identifies connect timeout as the network-phase timeout that callers use most. It
can remain a shared setting if each transport documents how it interprets the value.

Drop safety is the fallback for a deadline that a transport cannot express. When `fetch` cancels a
request future, the transport must stop and release its resources. See `fetch_winhttp` `design.md`
sections 6 and 7.

## 5. Transport construction ergonomics

The caller currently builds one dependency value before it builds the client:

```rust,ignore
HttpClient::builder_winhttp(WinHttpDeps::builder(clock, pool, sink).tls(cfg).build())
```

A fluent builder can keep transport configuration in one chain:

```rust,ignore
HttpClient::builder_winhttp()
    .tls(|tls| tls.accept_invalid_certs())
    .timeouts(|t| t.read(3000))
    .custom_pipeline(..)
    .build()
```

A transport builder can also plug into the client builder:

```rust,ignore
HttpClient::builder()
    .transport(WinHttpTransport::builder().tls(|tls| ..).build())
    .build()
```

Resolve this with the transport plug-in design from item 1. The current dependency value is
acceptable for version 1.

## 6. Telemetry sink as a `fetch` dependency

A transport needs an `observed::Sink` for events and metrics. `WinHttpDeps::sink` currently carries
it because `fetch` does not provide one.

Telemetry is shared pipeline infrastructure. `fetch` already owns the client meter. It can pass the
sink through the custom-transport context with the clock and memory pool. A transport then does not
need to expose a separate sink dependency.

## 7. Application and library configuration

Stabilization must decide who owns each setting when a library uses `fetch` for an application.

An application that calls an API directly can select the transport, TLS, certificate validation,
connection lifetime, and resilience policy.

A library that calls an endpoint for the application divides that responsibility. The review
considered three models:

1. The library owns the full client. The application cannot correct configuration and inherits the
   library transport and TLS dependencies.
2. The application builds the full `HttpClient` and gives it to the library. The library then loses
   control of settings such as its retry policy.
3. The application supplies the transport. The library configures its pipeline settings. The
   application can override selected properties.

The third model gives each side useful control. A fallback is the second model plus a library
function that configures an `HttpClientBuilder`.

Some library requirements do not fit the transport and pipeline split. Connection lifetime matters
to the ECS use case, but it is a transport setting. Early analysis says Hyper and WinHTTP can apply
the common settings that libraries need. The transport abstraction must expose more than a plain
layered or Tower service.

## 8. Shared surface with transport-specific behavior

`ConnectionPoolOptions`, `Http2Options`, and `ConnectionKeepAlive` remain on `TransportOptions`.
Their current behavior comes from `fetch_hyper`, so another transport cannot guarantee it.

`SocketOptions` is correctly on `TokioTransportOptions`. Only the bundled socket-owning transport
uses it.

No bundled transport currently reads `TransportOptions::extra`.
