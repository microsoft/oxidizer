// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The server side, which produces telemetry about incoming HTTP requests.

mod body_buffer;
mod drop_guard;
mod events;
mod header_fields;
mod interner;
mod media_types;
mod response_body;
mod route_fields;

pub(crate) use body_buffer::BodyBuffer;
pub(crate) use drop_guard::{HeaderValues, RequestAndResponseHeaders, RequestEnd, RouteValues, TelemetryDropGuard, UriAuthority};
pub(crate) use events::{ActiveRequests, RequestAbandoned, RequestComplete, RequestError};
pub(crate) use header_fields::{HeaderField, header_fields};
pub use media_types::MediaTypeError;
pub(crate) use media_types::MediaTypes;
pub(crate) use response_body::emit_when_body_ends;
pub(crate) use route_fields::RouteFields;

/// The replacement value for unknown HTTP methods, URL schemes, or HTTP protocol versions. We cannot support
/// arbitrary value there and must have finite lists to avoid dimension cardinality blow up.
pub(crate) const OTHER: &str = "_OTHER";
