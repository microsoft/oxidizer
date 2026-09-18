// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Loom models for allocator telemetry concurrency protocols.

#![cfg(loom)]

#[path = "../src/telemetry/remote_counts/batch_loom_tests.rs"]
mod remote_count_batches;
#[path = "../src/telemetry/remote_counts/loom_tests.rs"]
mod remote_counts;
mod tuning_telemetry;
