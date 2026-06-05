# fetch crate

## Conditional Compilation

**A TLS backend is mandatory to create an `HttpClient` for real network use.**
The `tokio` runtime alone is not enough — the hyper transport layer is built on top of
TLS, so both must be enabled together. There are currently two TLS backends, `rustls`
and `native-tls`, and more may be added. This produces the recurring cfg-guard pattern:

```rust
// "compile when the tokio runtime is selected AND a TLS backend is enabled"
#[cfg(all(feature = "tokio", any(feature = "rustls", feature = "native-tls")))]
```

When adding a new TLS backend, extend the inner `any(...)` accordingly (e.g. add the new
feature alongside `rustls` and `native-tls`).

Some items are guarded by a single TLS backend plus `test`, for example the TLS error
label resolution:

```rust
#[cfg(any(feature = "rustls", test))]
// ...
#[cfg(any(feature = "native-tls", test))]
```

`test-util` is the only escape hatch: it enables `FakeHandler` (canned responses, no
network), so no TLS backend is needed.
