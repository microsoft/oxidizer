// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The server side, which produces telemetry about incoming HTTP requests.

mod drop_guard;
mod events;
mod response_body;

pub(crate) use drop_guard::{RequestEnd, TelemetryDropGuard, UriAuthority};
pub(crate) use events::{ActiveRequests, RequestAbandoned, RequestComplete, RequestError};
pub(crate) use response_body::emit_when_body_ends;

/// The replacement value for unknown HTTP methods, URL schemes, or HTTP protocol versions. We cannot support
/// arbitrary value there and must have finite lists to avoid dimension cardinality blow up.
pub(crate) const OTHER: &str = "_OTHER";
