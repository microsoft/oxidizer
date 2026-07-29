<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Anyspawn Azure Logo" width="96">

# Anyspawn Azure

[![crate.io](https://img.shields.io/crates/v/anyspawn_azure.svg)](https://crates.io/crates/anyspawn_azure)
[![docs.rs](https://docs.rs/anyspawn_azure/badge.svg)](https://docs.rs/anyspawn_azure)
[![MSRV](https://img.shields.io/crates/msrv/anyspawn_azure)](https://crates.io/crates/anyspawn_azure)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Bundle [`anyspawn`][__link0] and [`tick`][__link1] as Azure SDK runtime abstractions.

The Azure SDK abstracts its task spawning, sleeping, and yielding behind the
[`azure_core::async_runtime::AsyncRuntime`][__link2] trait, and the process execution
that developer credentials rely on behind the `azure_identity::Executor`
trait. This crate adapts those primitives to both:

* [`Runtime`][__link3] implements `AsyncRuntime` on top of an [`anyspawn::Spawner`][__link4]
  (spawning) and a [`tick::Clock`][__link5] (sleeping).
* With the `azure-identity` feature, `Runtime` also implements
  `azure_identity::Executor`, running credential commands on the spawner.

## Example

```rust
use std::sync::Arc;

use anyspawn::Spawner;
use anyspawn_azure::Runtime;
use azure_core::async_runtime::{AsyncRuntime, set_async_runtime};
use tick::Clock;

// Install an `anyspawn`-backed async runtime (sleeping on a `tick::Clock`).
fn install_runtime(spawner: Spawner, clock: Clock) {
    let runtime: Arc<dyn AsyncRuntime> = Runtime::new(spawner, clock).into();
    let _ = set_async_runtime(runtime);
}
```


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/anyspawn_azure">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbG3R9HTN6WQcb7BPkFt-c1lwbFJyoKmJddDMbd44aiRZ9MR1hZISCaGFueXNwYXduZTAuNi4wgm5hbnlzcGF3bl9henVyZWUwLjEuM4JqYXp1cmVfY29yZWUxLjAuMIJkdGlja2UwLjQuMA
 [__link0]: https://crates.io/crates/anyspawn/0.6.0
 [__link1]: https://crates.io/crates/tick/0.4.0
 [__link2]: https://docs.rs/azure_core/1.0.0/azure_core/?search=async_runtime::AsyncRuntime
 [__link3]: https://docs.rs/anyspawn_azure/0.1.3/anyspawn_azure/?search=Runtime
 [__link4]: https://docs.rs/anyspawn/0.6.0/anyspawn/?search=Spawner
 [__link5]: https://docs.rs/tick/0.4.0/tick/?search=Clock
