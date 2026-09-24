// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(dead_code)]

mod contract {
    use renamed_metabench::__private::criterion::Criterion;
    use renamed_metabench::__private::gungraun::LibraryBenchmarkConfig;

    #[renamed_metabench::benchmark(FIRST, "group", "first")]
    fn first() {}

    #[renamed_metabench::benchmark(SECOND, "group", "second")]
    fn second() {}

    fn criterion_benchmarks(_: &mut Criterion) {}

    fn configured_criterion() -> Criterion {
        Criterion::default()
    }

    fn setup() {}
    fn teardown() {}

    renamed_metabench::main!(
        criterion = {
            factory = configured_criterion,
            benchmarks = criterion_benchmarks,
            unit = "cycles",
        },
        gungraun = {
            config = LibraryBenchmarkConfig::default();
            setup = setup();
            teardown = teardown();
        },
        groups = {
            COMPLETE {
                benchmarks = [FIRST, SECOND],
                gungraun_config = LibraryBenchmarkConfig::default(),
                gungraun_compare_by_id = true,
                gungraun_max_parallel = 1,
                gungraun_setup = setup(),
                gungraun_teardown = teardown(),
            },
        },
        allocator = std::alloc::System,
    );
}

fn main() {}
