// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for the hedging middleware in the `seatbelt` crate.
//!
//! Paired with `hedging.rs`, which covers the same happy-path operations under
//! wall-clock measurement. Each service is type-erased into a `DynamicService`
//! in the unmeasured `setup` step so the concrete (unnameable) service type can
//! be handed to the benchmark function. The dynamic dispatch adds a constant
//! per-call cost to every scenario, so it cancels out in the baseline-vs-hedging
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
    use seatbelt::hedging::Hedging;
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

    // Baseline: a bare service with no hedging wrapper.
    fn service_no_hedging() -> DynamicService<Input, Output> {
        Execute::new(|v: Input| async move { Output::from(v) }).into_dynamic()
    }

    // Hedging armed with a delay: the primary attempt completes before the
    // hedged attempt is ever launched, so the delay path is exercised.
    fn service_with_hedging_delay() -> DynamicService<Input, Output> {
        let context = ResilienceContext::new(Clock::new_frozen());
        (
            Hedging::layer("bench", &context)
                .clone_input()
                .recovery_with(|_, _| RecoveryInfo::never()),
            Execute::new(|v: Input| async move { Output::from(v) }),
        )
            .into_service()
            .into_dynamic()
    }

    // Hedging disabled (`max_hedged_attempts = 0`): the pass-through path with no
    // additional attempts ever scheduled.
    fn service_with_hedging_passthrough() -> DynamicService<Input, Output> {
        let context = ResilienceContext::new(Clock::new_frozen());
        (
            Hedging::layer("bench", &context)
                .clone_input()
                .recovery_with(|_, _| RecoveryInfo::never())
                .max_hedged_attempts(0),
            Execute::new(|v: Input| async move { Output::from(v) }),
        )
            .into_service()
            .into_dynamic()
    }

    #[library_benchmark]
    #[bench::no_hedging(service_no_hedging())]
    #[bench::with_hedging_delay(service_with_hedging_delay())]
    #[bench::with_hedging_passthrough(service_with_hedging_passthrough())]
    fn execute(service: DynamicService<Input, Output>) -> DynamicService<Input, Output> {
        black_box(block_on(service.execute(black_box(Input))));
        service
    }

    library_benchmark_group!(
        name = hedging;
        benchmarks = execute
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::hedging;

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default()
        .tool(Callgrind::with_args(["--branch-sim=yes"]));
    library_benchmark_groups = hedging
);
