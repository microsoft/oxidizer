// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

pub(crate) const CACHE_NAME: &str = "cache.name";

#[cfg_attr(not(test), expect(dead_code, reason = "used in tests to verify tracing field names"))]
pub(crate) const CACHE_EVENT_NAME: &str = "cache.event";

pub(crate) const CACHE_OPERATION_NAME: &str = "cache.operation";

pub(crate) const CACHE_ACTIVITY_NAME: &str = "cache.activity";

#[cfg_attr(not(test), expect(dead_code, reason = "used in tests to verify tracing field names"))]
pub(crate) const CACHE_DURATION_NAME: &str = "cache.duration_ns";
