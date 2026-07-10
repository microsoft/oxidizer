// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for the circuit breaker middleware in the `seatbelt`
//! crate.
//!
//! Paired with `breaker.rs`, which covers the same happy-path operations under
//! wall-clock measurement. Each service is type-erased into a `DynamicService`
//! in the unmeasured `setup` step so the concrete (unnameable) service type can
//! be handed to the benchmark function. The dynamic dispatch adds a constant
//! per-call cost to every scenario, so it cancels out in the baseline-vs-breaker
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

    use futures::executor::block_on;
    use gungraun::{library_benchmark, library_benchmark_group};
    use layered::{DynamicService, DynamicServiceExt, Execute, Service, Stack};
    use seatbelt::breaker::Breaker;
    use seatbelt::{RecoveryInfo, ResilienceContext};
    use tick::Clock;

    #[derive(Debug, Clone)]
    pub(super) struct Input;

    #[derive(Debug, Clone)]
    pub(super) struct Output;

    // Baseline: a bare service with no circuit breaker in front of it.
    fn service_no_breaker() -> DynamicService<Input, Result<Output, Output>> {
        Execute::new(|_input: Input| async move { Ok::<Output, Output>(Output) }).into_dynamic()
    }

    // Circuit breaker in the closed (pass-through) state: `min_throughput` is set
    // high enough that the breaker never trips, so every call flows to the inner
    // service.
    fn service_with_breaker() -> DynamicService<Input, Result<Output, Output>> {
        let context = ResilienceContext::new(Clock::new_frozen());
        (
            Breaker::layer("bench", &context)
                .recovery_with(|_, _| RecoveryInfo::never())
                .rejected_input_error(|_input, _args| Output)
                .min_throughput(1000),
            Execute::new(|_input: Input| async move { Ok(Output) }),
        )
            .into_service()
            .into_dynamic()
    }

    #[library_benchmark]
    #[bench::no_breaker(service_no_breaker())]
    #[bench::with_breaker(service_with_breaker())]
    fn execute(service: DynamicService<Input, Result<Output, Output>>) -> DynamicService<Input, Result<Output, Output>> {
        let _result = black_box(block_on(service.execute(black_box(Input))));
        service
    }

    library_benchmark_group!(
        name = breaker;
        benchmarks = execute
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::breaker;

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default()
        .tool(Callgrind::with_args(["--branch-sim=yes"]));
    library_benchmark_groups = breaker
);
