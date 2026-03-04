// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Type state markers for builder patterns.

/// A flag indicating that the required property is set.
#[non_exhaustive]
#[derive(Debug)]
pub struct Set;

/// A flag indicating that the required property has not been set.
#[non_exhaustive]
#[derive(Debug)]
pub struct NotSet;
