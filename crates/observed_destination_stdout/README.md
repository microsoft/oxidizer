<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Observed Destination Stdout Logo" width="96">

# Observed Destination Stdout

[![crate.io](https://img.shields.io/crates/v/observed_destination_stdout.svg)](https://crates.io/crates/observed_destination_stdout)
[![docs.rs](https://docs.rs/observed_destination_stdout/badge.svg)](https://docs.rs/observed_destination_stdout)
[![MSRV](https://img.shields.io/crates/msrv/observed_destination_stdout)](https://crates.io/crates/observed_destination_stdout)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

## Observed stdout Destination

Provides implementations of [`LogDestination`][__link0] and [`MetricDestination`][__link1] that write to stdout.

## Examples

```rust
use log::LevelFilter;
use observed_destination_core::{LogDestination, LoggerProviderBuilderExt};
use observed_destination_stdout::stdout_logs;
use opentelemetry::logs::LoggerProvider;
use opentelemetry_sdk::logs::SdkLoggerProvider;

let provider = SdkLoggerProvider::builder()
    .register(stdout_logs().level(LevelFilter::Warn))
    .unwrap()
    .build();

let logger = provider.logger("stdout");
```

```rust
use observed_destination_core::MeterProviderBuilderExt;
use opentelemetry::metrics::MeterProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;

let provider = SdkMeterProvider::builder()
    .register(observed_destination_stdout::stdout_metrics())
    .unwrap()
    .build();

provider
    .meter("stdout")
    .u64_counter("stdout_contract_requests")
    .build()
    .add(2, &[]);

provider.shutdown();
```


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/observed_destination_stdout">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbM-sN8Uca884by5Z3WXU-axIbD5ya2h_KhEQbirBsCqJg9hVhZIGCeBlvYnNlcnZlZF9kZXN0aW5hdGlvbl9jb3JlZjAuMjkuMA
 [__link0]: https://docs.rs/observed_destination_core/0.29.0/observed_destination_core/?search=LogDestination
 [__link1]: https://docs.rs/observed_destination_core/0.29.0/observed_destination_core/?search=MetricDestination
