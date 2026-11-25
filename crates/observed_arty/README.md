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

Integrates an Observed sink with the Arty runtime.

Call [`ObservedExt::enable_observed`][__link0] with an [`observed::Sink`][__link1] before you
submit a task. The first integration feature transfers enrichment into Arty
async tasks. It captures the enrichment at submission time, restores it for
each task poll, and removes it after the poll. Blocking functions submitted
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

Live registration runs immediately on the calling worker and is queued on
other workers. An async task submitted on another worker before it processes
that queue is not covered. Configure the runtime builder when every async
task must receive the integration.

Arty runs the task-spawn callback on the submitting worker when one exists.
Submissions from outside the runtime use Arty’s worker-neutral callback.
Arty moves the captured transfer to the target worker, creates the poll
observer there, and keeps that observer on the target worker.

Enrichment added inside the task stays above the transferred enrichment.
The integration restores the previous worker enrichment after every poll.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/observed_arty">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbYHtCfW0WwjsbM5SzdSb3T3AbAZDJNbA5sSgbwJ1wfKM0eL5hZIKCaG9ic2VydmVkZjAuMzAuMYJtb2JzZXJ2ZWRfYXJ0eWUwLjEuMA
 [__link0]: https://docs.rs/observed_arty/0.1.0/observed_arty/?search=ObservedExt::enable_observed
 [__link1]: https://docs.rs/observed/0.30.1/observed/?search=Sink
