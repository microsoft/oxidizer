// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(dead_code, non_snake_case)]

mod contract {
    use renamed_metabench::__private::criterion::Criterion;

    mod __metabench_native_COLLISION {}
    mod __metabench_group_module_COLLISION_GROUP {}
    mod __metabench_criterion {}
    mod __metabench_gungraun {}
    fn __metabench_benchmark_COLLISION() {}
    fn __metabench_benchmark_5ba7e6e02b0556f2d58ca7998a1efd17() {}
    const __METABENCH_IDENTITIES: () = ();
    static METABENCH_ALLOCATOR: () = ();

    #[renamed_metabench::benchmark(COLLISION, "collision", "collision")]
    fn collision() {}

    fn criterion_benchmarks(_: &mut Criterion) {}

    renamed_metabench::main!(
        criterion = criterion_benchmarks,
        groups = {
            COLLISION_GROUP {
                benchmarks = [COLLISION],
            },
        },
    );
}

fn main() {}

mod collector_names {
    use renamed_metabench::benchmark;

    #[benchmark(ALLOCATIONS, "collisions", "allocations")]
    #[bench::one(1)]
    fn allocations(value: u64) -> u64 {
        value + 1
    }

    #[benchmark(PERF, "collisions", "perf")]
    #[bench::one(1)]
    fn perf(value: u64) -> u64 {
        value + 2
    }

    #[benchmark(VTUNE, "collisions", "vtune")]
    #[bench::one(1)]
    fn vtune(value: u64) -> u64 {
        value + 3
    }

    #[benchmark(OUTPUT, "collisions", "output")]
    #[bench::one(1)]
    fn output(value: u64) -> u64 {
        value + 4
    }

    #[benchmark(ARGUMENT, "collisions", "argument")]
    #[bench::one(1)]
    fn __metabench_argument_0(value: u64) -> u64 {
        value + 5
    }

    #[benchmark(DROP, "collisions", "drop")]
    #[bench::one(1)]
    fn drop(value: u64) -> u64 {
        value + 6
    }
}
