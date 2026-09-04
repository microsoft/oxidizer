// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

struct Workload;

impl Workload {
    #[renamed_metabench::benchmark(IDENTITY, "group", "receiver")]
    fn workload(&self) {}
}

fn main() {}
