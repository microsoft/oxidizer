// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Utilities for testing `tracing` output without cross-test pollution.
//!
//! `tracing` subscribers and the `tracing-core` callsite-interest cache are
//! process-global state. This module installs a silent, always-interested fallback
//! subscriber (via [`initialize`]) so that no callsite can ever be poisoned into the
//! disabled state, and provides sanctioned ways to capture emitted events:
//! thread-local capture with [`Capture`] for unit tests, and the process-global
//! [`write_to_stdout_and_buffer`] bridge for `#[serial]` integration tests.
//!
//! See `docs/tracing-tests.md` for the full design and rules.

mod capture;
mod output;

pub use capture::*;
pub use output::*;
