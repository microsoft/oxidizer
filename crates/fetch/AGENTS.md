# `fetch` crate

## Conditional Compilation

A real-network `HttpClient` requires a TLS backend. The `tokio` feature alone is not
enough because the hyper transport requires TLS. Enable `rustls` or `native-tls` with
`tokio`. Use this conditional compilation pattern:

```rust
// Compile with Tokio and at least one TLS backend.
#[cfg(all(feature = "tokio", any(feature = "rustls", feature = "native-tls")))]
```

If you add a TLS backend, add its feature to the inner `any(...)`.

Some items use one TLS backend or `test`. TLS error labels use this pattern:

```rust
#[cfg(any(feature = "rustls", test))]
// ...
#[cfg(any(feature = "native-tls", test))]
```

`test-util` enables `FakeHandler`, which returns canned responses without network I/O.
It does not require a TLS backend.
