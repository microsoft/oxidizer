// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime assembly and worker startup.
//!
//! Startup selects worker placement and starts each asynchronous worker. It
//! collects all worker endpoints before it builds the dispatcher. It then
//! activates the workers and waits until every worker is ready.

mod startup;

pub(crate) use startup::build;
