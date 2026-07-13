// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for the telemetry overhead of the retry middleware in
//! the `seatbelt` crate.
//!
//! Paired with `observability.rs`, which covers the same happy-path operations
//! under wall-clock measurement, comparing no-telemetry, metrics-enabled, and
//! logs-enabled retry pipelines. Each service is type-erased into a
//! `DynamicService` in the unmeasured `setup` step so the concrete (unnameable)
//! service type can be handed to the benchmark function. The dynamic dispatch
//! adds a constant per-call cost to every scenario, so it cancels out in the
//! no-telemetry-vs-telemetry delta that these benchmarks exist to surface.

#![allow(missing_docs, reason = "no need for API documentation on benchmark code")]
#![allow(
    clippy::needless_pass_by_value,
    reason = "gungraun benchmark inputs are passed and returned by value by the framework"
)]
#![cfg_attr(
    target_os = "linux",
    expect(
        clippy::exit,
        clippy::missing_docs_in_private_items,
        unused_qualifications,
        reason = "Triggered by Gungraun macro expansion. Upstream tracking issues are pending."
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {
    // Gungraun requires Valgrind, which is Linux-only.
}

#[cfg(target_os = "linux")]
mod linux {
    use std::hint::black_box;
    use std::time::Duration;

    use futures::executor::block_on;
    use gungraun::{library_benchmark, library_benchmark_group};
    use layered::{DynamicService, DynamicServiceExt, Execute, Service, Stack};
    use opentelemetry_sdk::error::OTelSdkResult;
    use opentelemetry_sdk::metrics::data::ResourceMetrics;
    use opentelemetry_sdk::metrics::exporter::PushMetricExporter;
    use opentelemetry_sdk::metrics::{SdkMeterProvider, Temporality};
    use seatbelt::retry::Retry;
    use seatbelt::{RecoveryInfo, ResilienceContext};
    use tick::Clock;

    #[derive(Debug, Clone)]
    pub(super) struct Input;

    #[derive(Debug, Clone)]
    pub(super) struct Output;

    impl From<Input> for Output {
        fn from(_input: Input) -> Self {
            Self
        }
    }

    struct EmptyExporter;

    impl PushMetricExporter for EmptyExporter {
        async fn export(&self, _metrics: &ResourceMetrics) -> OTelSdkResult {
            Ok(())
        }

        fn force_flush(&self) -> OTelSdkResult {
            Ok(())
        }

        fn shutdown_with_timeout(&self, _timeout: Duration) -> OTelSdkResult {
            Ok(())
        }

        fn temporality(&self) -> Temporality {
            Temporality::Cumulative
        }
    }

    // Builds a retry pipeline over the given context. The context selects whether
    // metrics, logs, or neither are recorded on the resilience event path.
    fn retry_service(context: ResilienceContext<Input, Output>) -> DynamicService<Input, Output> {
        (
            Retry::layer("bench", &context)
                .clone_input()
                .base_delay(Duration::ZERO)
                .recovery_with(|_, _| RecoveryInfo::retry()),
            Execute::new(|v: Input| async move { Output::from(v) }),
        )
            .into_service()
            .into_dynamic()
    }

    // Baseline: retry with no telemetry sink configured.
    fn service_no_telemetry() -> DynamicService<Input, Output> {
        retry_service(ResilienceContext::new(Clock::new_frozen()))
    }

    // Retry emitting OpenTelemetry metrics to an empty (no-op) exporter. The
    // meter is cloned into the context, so the provider is dropped after setup.
    fn service_metrics() -> DynamicService<Input, Output> {
        let meter_provider = SdkMeterProvider::builder().with_periodic_exporter(EmptyExporter).build();
        retry_service(ResilienceContext::new(Clock::new_frozen()).use_metrics(&meter_provider))
    }

    // Retry emitting structured logs via the `tracing` integration.
    fn service_logs() -> DynamicService<Input, Output> {
        retry_service(ResilienceContext::new(Clock::new_frozen()).use_logs())
    }

    #[library_benchmark]
    #[bench::retry_no_telemetry(service_no_telemetry())]
    #[bench::retry_metrics(service_metrics())]
    #[bench::retry_logs(service_logs())]
    fn execute(service: DynamicService<Input, Output>) -> DynamicService<Input, Output> {
        black_box(block_on(service.execute(black_box(Input))));
        service
    }

    library_benchmark_group!(
        name = observability;
        benchmarks = execute
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::observability;

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default()
        .tool(Callgrind::with_args(["--branch-sim=yes"]));
    library_benchmark_groups = observability
);
