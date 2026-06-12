<div align="center">
 <img src="./logo.png" alt="Anyspawn Azure Logo" width="96">

# Anyspawn Azure

[![crate.io](https://img.shields.io/crates/v/anyspawn_azure.svg)](https://crates.io/crates/anyspawn_azure)
[![docs.rs](https://docs.rs/anyspawn_azure/badge.svg)](https://docs.rs/anyspawn_azure)
[![MSRV](https://img.shields.io/crates/msrv/anyspawn_azure)](https://crates.io/crates/anyspawn_azure)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Bundle [`anyspawn`][__link0] and [`tick`][__link1] as Azure SDK runtime abstractions.

The Azure SDK abstracts its task spawning, sleeping, and yielding behind the
[`typespec_client_core::async_runtime::AsyncRuntime`][__link2] trait, and the process
execution that developer credentials rely on behind the `azure_identity::Executor`
trait. This crate adapts those primitives to both:

* [`Runtime`][__link3] implements [`typespec_client_core::async_runtime::AsyncRuntime`][__link4] on top of
  an [`anyspawn::Spawner`][__link5] (spawning) and a [`tick::Clock`][__link6] (sleeping).
* With the `azure-identity` feature, [`Runtime`][__link7] also implements
  `azure_identity::Executor`, running credential commands on the
  [`anyspawn::Spawner`][__link8].

## Example

```rust
use std::sync::Arc;

use anyspawn::Spawner;
use anyspawn_azure::Runtime;
use tick::Clock;
use typespec_client_core::async_runtime::{AsyncRuntime, set_async_runtime};

// Install an `anyspawn`-backed async runtime (sleeping on a `tick::Clock`).
fn install_runtime(spawner: Spawner, clock: Clock) {
    let runtime: Arc<dyn AsyncRuntime> = Runtime::new(spawner, clock).into();
    let _ = set_async_runtime(runtime);
}
```


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/anyspawn_azure">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQbLiTyV0MU86EbZU15e0PmecoboQ9jo59bnAEbyDXw04U13GlhYvRhcoQbBIQZjhLbN24b4SQLJr0OB3sbAJBFhMk9gnYbZcuWw6vetXBhZISCaGFueXNwYXduZTAuNS4zgm5hbnlzcGF3bl9henVyZWUwLjEuMIJkdGlja2UwLjMuM4J0dHlwZXNwZWNfY2xpZW50X2NvcmVlMS4wLjA
 [__link0]: https://crates.io/crates/anyspawn/0.5.3
 [__link1]: https://crates.io/crates/tick/0.3.3
 [__link2]: https://docs.rs/typespec_client_core/1.0.0/typespec_client_core/?search=async_runtime::AsyncRuntime
 [__link3]: https://docs.rs/anyspawn_azure/0.1.0/anyspawn_azure/?search=Runtime
 [__link4]: https://docs.rs/typespec_client_core/1.0.0/typespec_client_core/?search=async_runtime::AsyncRuntime
 [__link5]: https://docs.rs/anyspawn/0.5.3/anyspawn/?search=Spawner
 [__link6]: https://docs.rs/tick/0.3.3/tick/?search=Clock
 [__link7]: https://docs.rs/anyspawn_azure/0.1.0/anyspawn_azure/?search=Runtime
 [__link8]: https://docs.rs/anyspawn/0.5.3/anyspawn/?search=Spawner
