<div align="center">
 <img src="./logo.png" alt="Observed Rt Logo" width="96">

# Observed Rt

[![crate.io](https://img.shields.io/crates/v/observed_rt.svg)](https://crates.io/crates/observed_rt)
[![docs.rs](https://docs.rs/observed_rt/badge.svg)](https://docs.rs/observed_rt)
[![MSRV](https://img.shields.io/crates/msrv/observed_rt)](https://crates.io/crates/observed_rt)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Context-propagating task spawner for [`observed`][__link0].

Wraps [`anyspawn::Spawner`][__link1] so that every spawned task (async or blocking)
automatically inherits the current enrichment state from a given sink.

See the [Enrichment][__link2] section in the `observed` crate for
background on how enrichment storage, scoping, and cross-thread transfer work.

## Example

```rust
use observed::enrichment::EnrichFutureExt;
use anyspawn::Spawner;

let sink = Sink::new(APP, vec![Arc::new(pipeline)], tick::SimpleClock::new_system());

async {
    let spawner = observed_rt::tokio(&sink);
    let handle = spawner.spawn(async {
        // enrichments from the spawn site are visible here
    });
    handle.await;
}
.enrich(&sink, [("request.id", "r-42")])
.await;
```


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/observed_rt">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQbLiTyV0MU86EbZU15e0PmecoboQ9jo59bnAEbyDXw04U13GlhYvRhcoQbPbzvjLoSz7EbIIdWDQp10L0boSpuwEV5JSUbcgx4FQGEF0ZhZIKCaGFueXNwYXduZTAuNi4wgmhvYnNlcnZlZGYwLjIzLjA
 [__link0]: https://crates.io/crates/observed/0.23.0
 [__link1]: https://docs.rs/anyspawn/0.6.0/anyspawn/?search=Spawner
 [__link2]: observed#enrichment
