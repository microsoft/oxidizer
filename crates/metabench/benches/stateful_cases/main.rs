// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Stateful, data-driven benchmarks in a multi-file executable.

mod hashmap;

metabench::main!(groups = [hashmap::HashMapBenchmarks], allocator = std::alloc::System,);
