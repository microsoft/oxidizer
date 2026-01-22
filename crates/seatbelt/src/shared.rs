// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// A flag indicating that the required property is set.
#[non_exhaustive]
#[derive(Debug)]
#[doc(hidden)]
pub struct Set;

/// A flag indicating that the required property has not been set.
#[non_exhaustive]
#[derive(Debug)]
#[doc(hidden)]
pub struct NotSet;
