// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(any(feature = "metrics", test))]
pub(crate) const CACHE_NAME: &str = "cache.name";

#[cfg(test)]
pub(crate) const CACHE_EVENT_NAME: &str = "cache.event";

#[cfg(any(feature = "metrics", test))]
pub(crate) const CACHE_OPERATION_NAME: &str = "cache.operation";

#[cfg(any(feature = "metrics", test))]
pub(crate) const CACHE_ACTIVITY_NAME: &str = "cache.activity";

#[cfg(test)]
pub(crate) const CACHE_DURATION_NAME: &str = "cache.duration_ns";
