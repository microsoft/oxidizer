<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Layered Logo" width="96">

# Layered

[![crate.io](https://img.shields.io/crates/v/layered.svg)](https://crates.io/crates/layered)
[![docs.rs](https://docs.rs/layered/badge.svg)](https://docs.rs/layered)
[![MSRV](https://img.shields.io/crates/msrv/layered)](https://crates.io/crates/layered)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

## Layered services

Build composable async services with layered middleware.

The [`Service`][__link0] trait defines an async operation. Layers wrap services to add behavior
such as timeouts, retries, and logging.

### Why not Tower?

[Tower][__link1] predates `async fn` in traits. Its services use explicit
`Future` types and `poll_ready` for back-pressure. They also use `&mut self`, so concurrent
requests can require service clones. This crate uses `async fn` with `&self`.
Enable the `tower-service` feature to use the Tower service adapters.

### Quick start

A [`Service`][__link2] asynchronously transforms an input into an output:

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

### Key concepts

* A [`Service`][__link4] is similar to `async fn(&self, In) -> Out`.
* Middleware wraps a service to add behavior before or after execution.
* A [`Layer`][__link5] creates middleware around a service.

### Layers and middleware

A [`Layer`][__link6] wraps a service with more behavior. This layer prints each input before it calls
the inner service:

```rust
use layered::{Execute, Layer, Service, Stack};

// Define a layer that adds logging.
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

// The first tuple element becomes the outer layer.
let service = (LogLayer, Execute::new(|x: i32| async move { x * 2 })).into_service();

let result = service.execute(21).await;
```

### Thread safety

Each service must implement [`Send`][__link7] and [`Sync`][__link8]. Each returned future must implement
[`Send`][__link9]. These bounds support multithreaded async runtimes such as Tokio.

### Features

* `intercept` enables [`Intercept`][__link10] middleware.
* `dynamic-service` enables the type-erased [`DynamicService`][__link11].
* `tower-service` enables Tower support through the [`tower`][__link12] module.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/layered">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbe3brLzpMi1obGPHAbdPVJrYbat9e_USDOP4bIvkdvp9GXy5hZIGCZ2xheWVyZWRlMC4zLjg
 [__link0]: https://docs.rs/layered/0.3.8/layered/?search=Service
 [__link1]: https://docs.rs/tower
 [__link10]: https://docs.rs/layered/0.3.8/layered/?search=Intercept
 [__link11]: https://docs.rs/layered/0.3.8/layered/?search=DynamicService
 [__link12]: https://docs.rs/layered/0.3.8/layered/tower/index.html
 [__link2]: https://docs.rs/layered/0.3.8/layered/?search=Service
 [__link3]: https://docs.rs/layered/0.3.8/layered/?search=Execute
 [__link4]: https://docs.rs/layered/0.3.8/layered/?search=Service
 [__link5]: https://docs.rs/layered/0.3.8/layered/?search=Layer
 [__link6]: https://docs.rs/layered/0.3.8/layered/?search=Layer
 [__link7]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link8]: https://doc.rust-lang.org/stable/std/marker/trait.Sync.html
 [__link9]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
