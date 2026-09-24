// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Loom models for allocator telemetry concurrency protocols.

#![cfg(loom)]

#[path = "loom_models/remote_count_batches.rs"]
mod remote_count_batches;
#[path = "loom_models/remote_counts.rs"]
mod remote_counts;
mod tuning_telemetry;
