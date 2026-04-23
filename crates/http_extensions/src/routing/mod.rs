// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Routing primitives for resolving the [`BaseUri`] of an outgoing request.
//!
//! A [`Routing`] decides which [`BaseUri`] should be attached to a target [`Uri`]
//! before it is sent. This is useful when a library or middleware needs to centralize
//! the resolution of the destination of HTTP requests while still allowing callers to
//! express the rest of the request (path, query, ...) independently.
//!
//! # Construction
//!
//! - [`Routing::default`] - returns no [`BaseUri`] (the target [`Uri`] is used as-is).
//! - [`Routing::base_uri`] - always returns the same [`BaseUri`].
//! - [`Routing::custom`] - delegates the decision to a user supplied closure that
//!   receives a [`RoutingContext`].
//!
//! # Conflict resolution
//!
//! When the target [`Uri`] passed to [`Routing::create_uri`] already carries a
//! [`BaseUri`] and the routing also produces one, [`Routing`] uses the configured
//! [`BaseUriConflict`] policy to decide what to do. The policy can be set with
//! [`Routing::conflict_policy`] and defaults to [`BaseUriConflict::KeepExisting`].

#[expect(unused_imports, reason = "simplifies the docs")]
use templated_uri::{BaseUri, Uri};

#[expect(
    clippy::module_inception,
    reason = "routing contains the `Routing` implementation, better to keep it in separate file"
)]
mod routing;
pub use routing::{BaseUriConflict, Routing};

mod routing_context;
pub use routing_context::RoutingContext;
