// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Longer guides for building applications with Arty.
//!
//! Start with the [quickstart](crate#quickstart), then choose a topic below.
//! These guides are compiled with all Arty features enabled. Ordinary examples
//! use [`arty::main`](crate::main) or [`arty::test`](crate::test); explicit runtime
//! construction is reserved for ownership, borrowing, and lifecycle examples.
//! API items retain their exact bounds, errors, and panic conditions; the guides
//! explain how those contracts fit together.

pub mod configuration;
pub mod lifecycle;
pub mod scheduling;
pub mod telemetry;
pub mod thread_awareness;
pub mod time;
