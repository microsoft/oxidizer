// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

fn criterion_benchmarks(_: &mut renamed_metabench::__private::criterion::Criterion) {}

mod contract {
    renamed_metabench::main!(
        criterion = super::criterion_benchmarks,
        benchmarks = [IDENTITY],
        groups = {
            GROUP { benchmarks = [IDENTITY] },
        },
    );
}

fn main() {}
