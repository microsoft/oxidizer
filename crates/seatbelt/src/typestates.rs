// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Type state markers for builder patterns.

/// Marks a required property as set.
#[non_exhaustive]
#[derive(Debug)]
pub struct Set;

/// Marks a required property as not set.
#[non_exhaustive]
#[derive(Debug)]
pub struct NotSet;
