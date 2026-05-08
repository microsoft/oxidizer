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

## Layered Services

Build composable async services with layered middleware.

This crate provides the [`Service`][__link0] trait and a layer system for adding cross-cutting
concerns like timeouts, retries, and logging.

### Why not Tower?

[Tower][__link1] predates `async fn` in traits, requiring manual `Future` types
or boxing and `poll_ready` back-pressure semantics. Tower’s `&mut self` also requires cloning
for concurrent requests. This crate uses `async fn` with `&self`, enabling simpler middleware
and natural concurrency. Tower interop is available via the `tower-service` feature.

### Quick Start

A [`Service`][__link2] transforms an input into an output asynchronously:

```rust
use layered::Service;

struct Greeter;

impl Service<String> for Greeter {
    type Out = String;

    async fn execute(&self, name: String) -> Self::Out {
        format!("Hello, {name}!")
    }
}
```

Use [`Execute`][__link3] to turn any async function into a service:

```rust
use layered::{Execute, Service};

let greeter = Execute::new(|name: String| async move { format!("Hello, {name}!") });

assert_eq!(greeter.execute("World".into()).await, "Hello, World!");
```

### Key Concepts

* **Service**: A type implementing the [`Service`][__link4] trait that transforms inputs into outputs
  asynchronously. Think of it as `async fn(&self, In) -> Out`.
* **Middleware**: A service that wraps another service to add cross-cutting behavior such as
  logging, timeouts, or retries. Middleware receives inputs before the inner service and can
  process outputs after.
* **Layer**: A type implementing the [`Layer`][__link5] trait that constructs middleware around a
  service. Layers are composable and can be stacked using tuples like `(layer1, layer2, service)`.

### Layers and Middleware

A [`Layer`][__link6] wraps a service with additional behavior. In this example, we create a logging
middleware that prints inputs before passing them to the inner service:

```rust
use layered::{Execute, Layer, Service, Stack};

// A simple logging layer
struct LogLayer;

impl<S> Layer<S> for LogLayer {
    type Service = LogService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        LogService(inner)
    }
}

struct LogService<S>(S);

impl<S, In: Send + std::fmt::Display> Service<In> for LogService<S>
where
    S: Service<In>,
{
    type Out = S::Out;

    async fn execute(&self, input: In) -> Self::Out {
        println!("Input: {input}");
        self.0.execute(input).await
    }
}

// Stack layers with the service (layers apply outer to inner)
let service = (LogLayer, Execute::new(|x: i32| async move { x * 2 })).into_service();

let result = service.execute(21).await;
```

### Thread Safety

All services must implement [`Send`][__link7] and [`Sync`][__link8], and returned futures must be [`Send`][__link9].
This ensures compatibility with multi-threaded async runtimes like Tokio.

### Features

* **`intercept`**: Enables [`Intercept`][__link10] middleware
* **`dynamic-service`**: Enables [`DynamicService`][__link11] for type erasure
* **`tower-service`**: Enables Tower interoperability via the [`tower`][__link12] module


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/layered">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEG9cd3_rKpXlxG3cPQgH_Ia06Gylje5wjfL8MG8IUl5cvZMADYWSBgmdsYXllcmVkZTAuMy4w
 [__link0]: https://docs.rs/layered/0.3.0/layered/?search=Service
 [__link1]: https://docs.rs/tower
 [__link10]: https://docs.rs/layered/0.3.0/layered/?search=Intercept
 [__link11]: https://docs.rs/layered/0.3.0/layered/?search=DynamicService
 [__link12]: https://docs.rs/layered/0.3.0/layered/tower/index.html
 [__link2]: https://docs.rs/layered/0.3.0/layered/?search=Service
 [__link3]: https://docs.rs/layered/0.3.0/layered/?search=Execute
 [__link4]: https://docs.rs/layered/0.3.0/layered/?search=Service
 [__link5]: https://docs.rs/layered/0.3.0/layered/?search=Layer
 [__link6]: https://docs.rs/layered/0.3.0/layered/?search=Layer
 [__link7]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link8]: https://doc.rust-lang.org/stable/std/marker/trait.Sync.html
 [__link9]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
