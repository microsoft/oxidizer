<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Observed Arty Logo" width="96">

# Observed Arty

[![crate.io](https://img.shields.io/crates/v/observed_arty.svg)](https://crates.io/crates/observed_arty)
[![docs.rs](https://docs.rs/observed_arty/badge.svg)](https://docs.rs/observed_arty)
[![MSRV](https://img.shields.io/crates/msrv/observed_arty)](https://crates.io/crates/observed_arty)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Transfers Observed enrichment into Arty async tasks.

Enrichment contains context fields that Observed attaches to emitted events.
The integration captures these fields from the thread that submits an async
task. For example, a request ID attached before submission remains visible
on events that the task emits on another worker.

Call [`ObservedExt::enable_observed`][__link0] with an [`observed::Sink`][__link1] before you
submit a task. The integration restores the captured enrichment for each
task poll and removes it after the poll. Blocking functions submitted
through `spawn_blocking` are not covered.

Enable the integration on a runtime builder:

```rust
use arty::runtime::Runtime;
use observed::Sink;
use observed_arty::ObservedExt as _;

let sink = Sink::builder("example").build();
let runtime = Runtime::builder().enable_observed(sink).build()?;
runtime.stop()?;
```

You can also enable the integration on a live runtime:

```rust
use arty::runtime::Runtime;
use observed::Sink;
use observed_arty::ObservedExt as _;

let runtime = Runtime::new()?;
let sink = Sink::builder("example").build();
runtime
    .scheduler()
    .block_on(async |cx| cx.control().enable_observed(sink))?;
runtime.stop()?;
```

Arty enables a live registration on the calling worker immediately. Each
other worker enables it when that worker processes the queued registration.
Until then, tasks submitted by that worker do not inherit enrichment through
this integration. Configure the runtime builder when every async task must
receive the integration.

The integration also captures enrichment when a thread outside the Arty
runtime submits a task. Captured enrichment appears before enrichment added
inside the task. The integration restores the previous worker enrichment
after every poll.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/observed_arty">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbxJyfb6wxy1cbgQuXvTQLGQcbg_dk91UtVBwbEelBqUOyy89hZIKCaG9ic2VydmVkZjAuMzEuMIJtb2JzZXJ2ZWRfYXJ0eWUwLjEuMA
 [__link0]: https://docs.rs/observed_arty/0.1.0/observed_arty/?search=ObservedExt::enable_observed
 [__link1]: https://docs.rs/observed/0.31.0/observed/?search=Sink
