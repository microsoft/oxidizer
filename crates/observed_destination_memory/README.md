<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Observed Destination Memory Logo" width="96">

# Observed Destination Memory

[![crate.io](https://img.shields.io/crates/v/observed_destination_memory.svg)](https://crates.io/crates/observed_destination_memory)
[![docs.rs](https://docs.rs/observed_destination_memory/badge.svg)](https://docs.rs/observed_destination_memory)
[![MSRV](https://img.shields.io/crates/msrv/observed_destination_memory)](https://crates.io/crates/observed_destination_memory)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

In-memory log and metric destinations for testing.

Use these destinations in tests that make sure that code emits the expected logs
and metrics. Each destination sends the data to an `opentelemetry_sdk` in-memory
exporter. Clones of an exporter share one buffer. Keep a clone and read the
captured data from it.

Add this crate to `[dev-dependencies]`. It turns on the `testing` feature of
`opentelemetry_sdk`. Cargo merges the features that all crates in a build request
for a dependency. A normal dependency on this crate thus turns on `testing` for
every user of `opentelemetry_sdk` in that build.

## Examples

```rust
use observed_destination_core::LoggerProviderBuilderExt;
use observed_destination_memory::memory_logs;
use opentelemetry::logs::{LogRecord as _, Logger as _, LoggerProvider as _};
use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider};

let exporter = InMemoryLogExporter::default();
let provider = SdkLoggerProvider::builder()
    .register(memory_logs(exporter.clone()))?
    .build();

let logger = provider.logger("example");
let mut record = logger.create_log_record();
record.set_body("request received".into());
logger.emit(record);

assert_eq!(exporter.get_emitted_logs()?.len(), 1);
```


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/observed_destination_memory">source code</a>.
</sub>

