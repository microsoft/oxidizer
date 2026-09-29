// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Submission capabilities and completion semantics.
//!
//! Schedulers submit work. Task execution prepares
//! wrappers and result transport; scheduling and runtime worker components decide
//! placement and admission and execute accepted work. Admission determines whether
//! submitted work is accepted, for example during shutdown. Join handles consume
//! the resulting value or panic notification rather than producing the result.

pub(crate) mod execution;
pub(crate) mod join;
pub(crate) mod local;
pub(crate) mod scheduler;
