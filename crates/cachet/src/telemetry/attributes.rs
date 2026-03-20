// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(any(feature = "metrics", test))]
pub(crate) const CACHE_NAME: &str = "cache.name";

#[cfg(any(feature = "metrics", test))]
pub(crate) const CACHE_OPERATION_NAME: &str = "cache.operation";

#[cfg(any(feature = "metrics", test))]
pub(crate) const CACHE_ACTIVITY_NAME: &str = "cache.activity";
