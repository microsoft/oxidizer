// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Router primitives for resolving the [`BaseUri`] of an outgoing request.
//!
//! A [`Router`] decides which [`BaseUri`] should be attached to a target [`Uri`]
//! before it is sent. This is useful when a library or middleware needs to centralize
//! the resolution of the destination of HTTP requests while still allowing callers to
//! express the rest of the request (path, query, ...) independently.
//!
//! # Construction
//!
//! - [`Router::default`] - returns no [`BaseUri`] (the target [`Uri`] is used as-is).
//! - [`Router::fixed`] - always resolves to the same [`BaseUri`].
//! - [`Router::fallback`] - selects between a primary and a fallback [`BaseUri`]
//!   based on the previous attempt's recovery information and the current
//!   attempt's position in the retry sequence.
//! - [`Router::custom`] - delegates the decision to a user supplied closure that
//!   receives a [`RouterContext`].
//!
//! # Conflict resolution
//!
//! When the target [`Uri`] passed to [`Router::resolve_uri`] already carries a
//! [`BaseUri`] and the routing also produces one, [`Router`] uses the configured
//! [`BaseUriConflict`] policy to decide what to do. The policy can be set with
//! [`Router::conflict_policy`] and defaults to [`BaseUriConflict::UseOriginal`].

#[expect(unused_imports, reason = "simplifies the docs")]
use templated_uri::{BaseUri, Uri};

mod router;
pub use router::{BaseUriConflict, Router};

mod router_context;
pub use router_context::RouterContext;
