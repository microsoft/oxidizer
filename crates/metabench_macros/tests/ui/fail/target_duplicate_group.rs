// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

fn criterion_benchmarks(_: &mut renamed_metabench::__private::criterion::Criterion) {}

mod contract {
    renamed_metabench::main!(
        criterion = super::criterion_benchmarks,
        groups = {
            GROUP { benchmarks = [FIRST] },
            GROUP { benchmarks = [SECOND] },
        },
    );
}

fn main() {}
