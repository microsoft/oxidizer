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
    fn __metabench_benchmark_434f4c4c4953494f4e() {}
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
