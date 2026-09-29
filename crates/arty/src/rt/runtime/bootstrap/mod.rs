// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime assembly and the two-sided worker startup protocol.

pub(in crate::rt::runtime) mod pools;
mod startup;

pub(in crate::rt::runtime) use startup::build;
