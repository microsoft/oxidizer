// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Chaos engineering middleware for testing resilience under failure conditions.
//!
//! This module provides middleware that deliberately injects faults into a
//! service pipeline, enabling teams to verify that their systems handle
//! failures gracefully.
//!
//! ## Available Middleware
//!
//! - [`injection`] - Replaces service output with a user-provided value at a
//!   configurable probability.

#[cfg(any(feature = "chaos-injection", test))]
pub mod injection;
