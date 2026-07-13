// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for the timeout middleware in the `seatbelt` crate.
//!
//! Paired with `timeout.rs`, which covers the same happy-path operations under
//! wall-clock measurement. Each service is type-erased into a `DynamicService`
//! in the unmeasured `setup` step so the concrete (unnameable) service type can
//! be handed to the benchmark function. The dynamic dispatch adds a constant
//! per-call cost to every scenario, so it cancels out in the baseline-vs-timeout
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
    use seatbelt::ResilienceContext;
    use seatbelt::timeout::Timeout;
    use tick::Clock;

    pub(super) struct Input;

    pub(super) struct Output;

    impl From<Input> for Output {
        fn from(_input: Input) -> Self {
            Self
        }
    }

    // Baseline: a bare service with no timeout wrapper.
    fn service_no_timeout() -> DynamicService<Input, Output> {
        Execute::new(|v: Input| async move { Output::from(v) }).into_dynamic()
    }

    // Timeout in the happy path: the inner service completes well within the
    // (generous) timeout, so the deadline never fires.
    fn service_with_timeout() -> DynamicService<Input, Output> {
        let context = ResilienceContext::new(Clock::new_frozen());
        (
            Timeout::layer("bench", &context)
                .timeout_output(|_args| Output)
                .timeout(Duration::from_secs(10)),
            Execute::new(|v: Input| async move { Output::from(v) }),
        )
            .into_service()
            .into_dynamic()
    }

    #[library_benchmark]
    #[bench::no_timeout(service_no_timeout())]
    #[bench::with_timeout(service_with_timeout())]
    fn execute(service: DynamicService<Input, Output>) -> DynamicService<Input, Output> {
        black_box(block_on(service.execute(black_box(Input))));
        service
    }

    library_benchmark_group!(
        name = timeout;
        benchmarks = execute
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::timeout;

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default()
        .tool(Callgrind::with_args(["--branch-sim=yes"]));
    library_benchmark_groups = timeout
);
