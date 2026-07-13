// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for the retry middleware in the `seatbelt` crate.
//!
//! Paired with `retry.rs`, which covers the same happy-path operations under
//! wall-clock measurement. Each service is type-erased into a `DynamicService`
//! in the unmeasured `setup` step so the concrete (unnameable) service type can
//! be handed to the benchmark function. The dynamic dispatch adds a constant
//! per-call cost to every scenario, so it cancels out in the baseline-vs-retry
//! delta that these benchmarks exist to surface.

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

    // Baseline: a bare service with no retry wrapper.
    fn service_no_retry() -> DynamicService<Input, Output> {
        Execute::new(|v: Input| async move { Output::from(v) }).into_dynamic()
    }

    // Retry configured to never recover: the inner service succeeds on the first
    // attempt, so no retry is ever scheduled.
    fn service_with_retry() -> DynamicService<Input, Output> {
        let context = ResilienceContext::new(Clock::new_frozen());
        (
            Retry::layer("bench", &context)
                .clone_input()
                .recovery_with(|_, _| RecoveryInfo::never()),
            Execute::new(|v: Input| async move { Output::from(v) }),
        )
            .into_service()
            .into_dynamic()
    }

    // Retry configured to recover: exercises the recovery-decision path with a
    // zero base delay and a single permitted attempt.
    fn service_with_retry_and_recovery() -> DynamicService<Input, Output> {
        let context = ResilienceContext::new(Clock::new_frozen());
        (
            Retry::layer("bench", &context)
                .clone_input()
                .max_retry_attempts(1)
                .base_delay(Duration::ZERO)
                .recovery_with(|_, _| RecoveryInfo::retry()),
            Execute::new(|v: Input| async move { Output::from(v) }),
        )
            .into_service()
            .into_dynamic()
    }

    #[library_benchmark]
    #[bench::no_retry(service_no_retry())]
    #[bench::with_retry(service_with_retry())]
    #[bench::with_retry_and_recovery(service_with_retry_and_recovery())]
    fn execute(service: DynamicService<Input, Output>) -> DynamicService<Input, Output> {
        black_box(block_on(service.execute(black_box(Input))));
        service
    }

    library_benchmark_group!(
        name = retry;
        benchmarks = execute
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::retry;

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default()
        .tool(Callgrind::with_args(["--branch-sim=yes"]));
    library_benchmark_groups = retry
);
