// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

fn criterion_benchmarks(_: &mut renamed_metabench::__private::criterion::Criterion) {}

mod contract {
    renamed_metabench::main!(
        criterion = super::criterion_benchmarks,
        groups = {
            GROUP {
                benchmarks = [IDENTITY],
                gungraun_setup = first,
                gungraun_setup = second,
            },
        },
    );
}

fn main() {}
