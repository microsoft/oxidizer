# Examples

Runnable examples covering each middleware and common composition patterns:

- [`timeout`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/timeout.rs): Basic timeout that cancels long-running operations.
- [`timeout_advanced`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/timeout_advanced.rs): Dynamic timeout durations and timeout callbacks.
- [`retry`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/retry.rs): Automatic retry with input cloning and recovery classification.
- [`retry_advanced`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/retry_advanced.rs): Custom input cloning with attempt metadata injection.
- [`retry_outage`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/retry_outage.rs): Input restoration from errors when cloning is not possible.
- [`breaker`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/breaker.rs): Circuit breaker that monitors failure rates.
- [`fallback`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/fallback.rs): Substitutes default values for invalid outputs.
- [`resilience_pipeline`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/resilience_pipeline.rs): Composing retry and timeout with metrics.
- [`tower`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/tower.rs): Tower `ServiceBuilder` integration.
- [`config`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/config.rs): Loading settings from a [JSON file](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/config.json).
