# Examples

These examples show each middleware and common ways to combine them:

- [`timeout`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/timeout.rs): Stops operations that take too long.
- [`timeout_advanced`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/timeout_advanced.rs): Uses a dynamic timeout and timeout callbacks.
- [`retry`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/retry.rs): Retries operations with cloned input and output classification.
- [`retry_advanced`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/retry_advanced.rs): Clones input and adds attempt data.
- [`retry_outage`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/retry_outage.rs): Restores input from an error when the input cannot be cloned.
- [`breaker`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/breaker.rs): Monitors failure rates and opens a circuit.
- [`hedging`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/hedging.rs): Starts parallel attempts when a request is slow.
- [`fallback`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/fallback.rs): Replaces invalid output with a default value.
- [`resilience_pipeline`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/resilience_pipeline.rs): Combines retry and timeout middleware with metrics.
- [`tower`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/tower.rs): Uses Seatbelt with Tower `ServiceBuilder`.
- [`chaos_injection`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/chaos_injection.rs): Replaces output at a configured rate.
- [`chaos_latency`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/chaos_latency.rs): Adds delay at a configured rate.
- [`config`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/config.rs): Loads configuration from a [JSON file](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/config.json).
