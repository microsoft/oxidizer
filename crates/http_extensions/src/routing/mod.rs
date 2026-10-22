// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Router primitives for resolving the [`BaseUri`] of an outgoing request.
//!
//! A [`Router`] selects the [`BaseUri`] for a target [`Uri`] before a request is sent.
//! A base URI contains a scheme and authority, such as `https://api.example.com`.
//! A library or middleware can use one router to select request destinations.
//! Callers can still define the path and query for each request.
//!
//! For example, a service can run in two regions:
//! `https://api-westus.example.com` and `https://api-eastus.example.com`.
//! Requests normally use the primary region.
//! If that region becomes unavailable, [`Router::fallback`] selects the secondary region
//! for the next attempt.
//!
//! # Construction
//!
//! - [`Router::default`] creates a router without a [`BaseUri`]. It keeps the target [`Uri`] unchanged.
//! - [`Router::fixed`] creates a router that always selects the same [`BaseUri`].
//! - [`Router::fallback`] creates a router with a primary and fallback [`BaseUri`].
//!   It uses recovery information and the current attempt position.
//! - [`Router::custom`] delegates the decision to a user-supplied closure.
//!   The closure receives a [`RouterContext`].
//!
//! # Conflict resolution
//!
//! When the target [`Uri`] passed to [`Router::resolve_uri`] already carries a
//! [`BaseUri`] and routing also produces one, [`Router`] uses the selected
//! [`BaseUriConflict`] policy. Set the policy with [`Router::conflict_policy`].
//! The default is [`BaseUriConflict::UseOriginal`].
//!
//! # Recovery Context
//!
//! A [`Router`] does not perform recovery.
//! It resolves a [`BaseUri`] for one attempt.
//! If a routing decision depends on previous attempts, pass that state through
//! a [`RouterContext`].
//! The context contains the attempt index, the last-attempt flag, and the previous
//! [`RecoveryInfo`](recoverable::RecoveryInfo).
//!
//! The caller populates the [`RouterContext`].
//! A recovery policy around the HTTP client typically performs this task.
//! Before each attempt, it builds a new context and calls [`Router::resolve_request_uri`].
//! If the client has no recovery layer, use [`RouterContext::new`].
//!
//! [`RecoveryInfo`](recoverable::RecoveryInfo) describes recovery for the previous failure.
//! In particular,
//! [`RecoveryKind::Unavailable`](recoverable::RecoveryKind::Unavailable)
//! means that the endpoint does not serve traffic.
//! Examples include an open circuit breaker and a `503 Service Unavailable` response.
//! [`Router::fallback`] uses this value to select another endpoint for the next attempt.
//!
//! [`BaseUri`]: templated_uri::BaseUri
//! [`Uri`]: templated_uri::Uri

mod router;
pub(crate) use router::RenderedPath;
pub use router::{BaseUriConflict, RequestUris, Router};

mod router_context;
pub use router_context::RouterContext;
