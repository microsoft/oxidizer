<div align="center">
 <img src="./logo.png" alt="Layered Logo" width="96">

# Layered

[![crate.io](https://img.shields.io/crates/v/layered.svg)](https://crates.io/crates/layered)
[![docs.rs](https://docs.rs/layered/badge.svg)](https://docs.rs/layered)
[![MSRV](https://img.shields.io/crates/msrv/layered)](https://crates.io/crates/layered)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

## Service Composition Framework

A foundational service abstraction for building composable, middleware-driven systems.
This crate provides the [`Service`][__link0] trait and layer composition system that enables systematic
application of cross-cutting concerns such as timeouts, retries, and observability.

## Why

This crate enables easy composability using modern Rust features. Why not just use
[Tower][__link1]? Tower predates `async fn` in traits, requiring boxed futures and
`poll_ready` semantics that add complexity we don’t need. This crate provides a simpler
`execute`-based model with cleaner trait bounds, while still offering Tower interoperability
via the `tower-service` feature.

## Quickstart

A basic service transforms an input into an output:

```rust
use std::future::Future;

use layered::Service;

struct DatabaseService;

impl Service<String> for DatabaseService {
    type Out = Vec<u8>;

    async fn execute(&self, query: String) -> Self::Out {
        // Simulate database query execution
        format!("SELECT * FROM users WHERE name = '{}'", query).into_bytes()
    }
}
```

### Key Concepts

* **Service**: An async function `In → Future<Out>` that processes inputs. All services
  implement the [`Service`][__link2] trait.
* **Middleware**: A service wrapper that adds cross-cutting functionality (logging, timeouts, retries)
  before delegating to an inner service. Middleware also implements the [`Service`][__link3] trait.
* **Layer**: A factory that wraps any service with middleware functionality. Multiple layers can
  be combined using tuple syntax like `(timeout, retry, core_service)` to create an execution stack
  where middleware is applied in order, with the core service at the bottom.

### Middleware

Services can be composed by wrapping them with additional services. Middleware services
add functionality such as logging, metrics, or error handling, then call the inner service.

```rust
use layered::Service;

struct Logging<S> {
    inner: S,
    name: &'static str,
}

impl<S, In: Send> Service<In> for Logging<S>
where
    S: Service<In>,
    In: std::fmt::Debug,
    S::Out: std::fmt::Debug,
{
    type Out = S::Out;

    async fn execute(&self, input: In) -> Self::Out {
        println!("{}: Processing input: {:?}", self.name, input);
        let output = self.inner.execute(input).await;
        println!("{}: Output: {:?}", self.name, output);
        output
    }
}
```

## Layer and Composition

For systematic middleware composition, use the [`crate::Layer`][__link4]. Layers are builders
for middleware services that can be applied to any service. This allows you to create reusable
and composable middleware.

```rust
use layered::{Execute, Layer, Service, ServiceBuilder};

// The middleware service
pub struct Timeout<S> {
    inner: S,
    timeout: std::time::Duration,
}

impl Timeout<()> {
    // By convention, layers are created using a `layer` method exposed
    // by the middleware.
    pub fn layer(timeout: std::time::Duration) -> TimeoutLayer {
        TimeoutLayer { timeout }
    }
}

// Middleware implements the `Service` trait
impl<S, In: Send> Service<In> for Timeout<S>
where
    S: Service<In>,
{
    type Out = Result<S::Out, &'static str>;

    async fn execute(&self, input: In) -> Self::Out {
        // In a real implementation, this would use a proper timeout mechanism
        Ok(self.inner.execute(input).await)
    }
}

// Actual layer that is able to wrap inner service with logging functionality
pub struct TimeoutLayer {
    timeout: std::time::Duration,
}

// Layer must be implemented
impl<S> Layer<S> for TimeoutLayer {
    type Service = Timeout<S>;

    fn layer(&self, inner: S) -> Self::Service {
        Timeout {
            inner,
            timeout: self.timeout,
        }
    }
}


// Define the layers and the root service
let execution_stack = (
    Timeout::layer(std::time::Duration::from_secs(5)),
    Execute::new(|input: String| async move { input }),
);

// Build the service with the layers applied
let service = execution_stack.build();

// Execute an input
let output = service.execute("hello".to_string()).await;
```

## Thread Safety and Concurrency

All [`Service`][__link5] implementations must be [`Send`][__link6] and [`Sync`][__link7], enabling safe use across
threads and async runtimes. This is essential because:

* **Multi-threaded runtimes**: Services may be called from different threads in runtimes such as Tokio
* **Concurrent inputs**: Multiple inputs may be processed simultaneously using the same service
* **Shared state**: Services are often shared between different parts of an application

The returned future must also be [`Send`][__link8], ensuring it can be moved between threads during
async execution. This enables services to work seamlessly in both single-threaded
(thread-per-core) and multi-threaded runtime environments.

## Built-in Services and Middleware

* **[`Execute`][__link9]**: Converts any function or closure into a service. Always available.
* **`Intercept`**: Middleware for observing and modifying service inputs and outputs.
  Useful for logging, debugging, and validation. Requires the `intercept` feature.
* **`DynamicService`**: Type-erased service wrapper for hiding concrete service types.
  Useful for complex compositions and collections. Requires the `dynamic-service` feature.

## Tower Service Interoperability

This crate provides seamless interoperability with the Tower ecosystem through the `tower-service` feature.
When enabled, you can:

* Convert between oxidizer [`Service`][__link10] and Tower’s `tower::Service` trait
* Use existing Tower middleware with oxidizer services
* Integrate oxidizer services into Tower-based applications

The `tower` module contains all Tower-related functionality and is only available
when the `tower-service` feature is enabled.

## Features

This crate supports the following optional features:

* **`intercept`**: Enables the `Intercept` middleware for debugging and observability
* **`dynamic-service`**: Enables `DynamicService` and `DynamicServiceExt` for type-erased services
* **`tower-service`**: Enables interoperability with the Tower ecosystem via the `tower` module


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/layered">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEG49hfJQcwZOWG5CxleatVy08GxwLsUqpUbSnGzwajOPVeJToYWSBgmdsYXllcmVkZTAuMS4w
 [__link0]: https://docs.rs/layered/0.1.0/layered/?search=Service
 [__link1]: https://docs.rs/tower
 [__link10]: https://docs.rs/layered/0.1.0/layered/?search=Service
 [__link2]: https://docs.rs/layered/0.1.0/layered/?search=Service
 [__link3]: https://docs.rs/layered/0.1.0/layered/?search=Service
 [__link4]: https://docs.rs/layered/0.1.0/layered/?search=Layer
 [__link5]: https://docs.rs/layered/0.1.0/layered/?search=Service
 [__link6]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link7]: https://doc.rust-lang.org/stable/std/marker/trait.Sync.html
 [__link8]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link9]: https://docs.rs/layered/0.1.0/layered/?search=Execute
