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
    use seatbelt::breaker::{Breaker, BreakerId};
    use seatbelt::{RecoveryInfo, ResilienceContext};
    use tick::Clock;

    #[derive(Debug, Clone)]
    pub(super) struct Input(u64);

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

    // Builds a partitioned circuit breaker whose ID provider keys on the request's
    // partition. Returned un-warmed so each scenario can create the partitions it needs.
    fn build_partitioned() -> impl Service<Input, Out = Result<Output, Output>> + 'static {
        let context = ResilienceContext::new(Clock::new_frozen());
        (
            Breaker::layer("bench", &context)
                .breaker_id(|input: &Input| BreakerId::from(input.0))
                .recovery_with(|_, _| RecoveryInfo::never())
                .rejected_input_error(|_input, _args| Output)
                .min_throughput(1000),
            Execute::new(|_input: Input| async move { Ok(Output) }),
        )
            .into_service()
    }

    // Creates an engine for each partition in `partitions`, then type-erases the service.
    // The measured `execute` call always targets partition 0.
    fn warmed(partitions: impl IntoIterator<Item = u64>) -> DynamicService<Input, Result<Output, Output>> {
        let service = build_partitioned();
        for partition in partitions {
            let _warm = block_on(service.execute(Input(partition)));
        }
        service.into_dynamic()
    }

    // Single partition: every request targets the same authority (the common case). The
    // measured call resolves the sole existing engine.
    fn service_with_partitioned() -> DynamicService<Input, Result<Output, Output>> {
        warmed(0..1)
    }

    // A moderate number of partitions already exist; the measured call resolves one of them.
    fn service_with_partitioned_many() -> DynamicService<Input, Result<Output, Output>> {
        warmed(0..16)
    }

    // A large, high-cardinality partition set (an anti-pattern the docs discourage, included
    // to bound the worst case): confirms the lookup stays cheap as the map grows.
    fn service_with_partitioned_large() -> DynamicService<Input, Result<Output, Output>> {
        warmed(0..256)
    }

    // Miss: partitions 1..=16 exist but partition 0 (the measured request) does not, so the
    // call falls to the write-lock path that creates and inserts a new engine. This one-shot
    // insert has no Criterion counterpart: a Criterion loop would insert only on its first
    // iteration and hit thereafter, so it cannot repeatably measure the miss.
    fn service_with_partitioned_miss() -> DynamicService<Input, Result<Output, Output>> {
        warmed(1..=16)
    }

    #[library_benchmark]
    #[bench::no_breaker(service_no_breaker())]
    #[bench::with_breaker(service_with_breaker())]
    #[bench::with_partitioned(service_with_partitioned())]
    #[bench::with_partitioned_many(service_with_partitioned_many())]
    #[bench::with_partitioned_large(service_with_partitioned_large())]
    #[bench::with_partitioned_miss(service_with_partitioned_miss())]
    fn execute(service: DynamicService<Input, Result<Output, Output>>) -> DynamicService<Input, Result<Output, Output>> {
        let _result = black_box(block_on(service.execute(black_box(Input(0)))));
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
