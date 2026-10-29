# `fetch` crate

## Conditional Compilation

A real-network `HttpClient` requires a TLS backend.
The `arty` and `tokio` features do not enable TLS. Enable one runtime feature and
one TLS feature. There are two TLS backends, `rustls` and `native-tls`.

```rust
// Compile the Tokio integration when a TLS backend is enabled.
#[cfg(all(feature = "tokio", any(feature = "rustls", feature = "native-tls")))]

// Compile the Arty integration when a TLS backend is enabled.
#[cfg(all(feature = "arty", any(feature = "rustls", feature = "native-tls")))]
```

If you add a TLS backend, add its feature to each applicable `any(...)` expression.

Some items use one TLS backend or `test`. TLS error labels use this pattern:

```rust
#[cfg(any(feature = "rustls", test))]
// ...
#[cfg(any(feature = "native-tls", test))]
```

`test-util` enables `FakeHandler`, which returns canned responses without network I/O.
It does not require a TLS backend.
