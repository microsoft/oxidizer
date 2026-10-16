// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime assembly and the two-sided worker startup protocol.

pub(crate) mod pools;
mod startup;

pub(crate) use startup::build;
