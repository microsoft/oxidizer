// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Chaos middleware for testing failures and delays.
//!
//! This middleware adds controlled faults to a service pipeline.
//! Use it to test how the pipeline handles failures.
//!
//! ## Available middleware
//!
//! - [`injection`] replaces service output at a configured rate.
//! - [`latency`] adds a delay before the inner service call at a configured rate.

#[cfg(any(feature = "chaos-injection", test))]
pub mod injection;

#[cfg(any(feature = "chaos-latency", test))]
pub mod latency;
