// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Guides for running applications with Arty.
//!
//! Start with the [quickstart](crate#quickstart), then choose a topic below.
//!
//! - [Scheduling](scheduling): create tasks, share local state, and receive results.
//! - [Configuration](configuration): choose workers, blocking pools, and services.
//! - [Shutdown](shutdown): stop the runtime, cancel pending tasks, and wait for workers.
//! - [Thread awareness](thread_awareness): preserve or change a worker association.
//! - [Time](time): use delays and timeouts, and control time in tests.
//! - [Telemetry](telemetry): configure events, enrichment, and data classification.
//!
//! This module is only included in docs.rs documentation builds, not in the
//! public API available to applications.

pub mod configuration;
pub mod scheduling;
pub mod shutdown;
pub mod telemetry;
pub mod thread_awareness;
pub mod time;
