// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Router primitives for resolving the [`BaseUri`] of an outgoing request.
//!
//! A [`Router`] decides which [`BaseUri`] (scheme + authority, e.g.
//! `https://api.example.com`) should be attached to a target [`Uri`] before it
//! is sent. This is useful when a library or middleware needs to centralize
//! the resolution of the destination of HTTP requests while still allowing callers to
//! express the rest of the request (path, query, ...) independently.
//!
//! Typical scenario for a router can be a service that is deployed to two regions,
//! `https://api-westus.example.com` and `https://api-eastus.example.com`.
//! Requests normally target the primary region, but when it becomes unavailable the [`Router`]
//! swaps in the secondary region on the next retry so the call can still succeed. See
//! [`Router::fallback`].
//!
//! # Construction
//!
//! - [`Router::default`] - creates a [`Router`] that resolves to no [`BaseUri`] (the target [`Uri`] is used as-is).
//! - [`Router::fixed`] - creates a [`Router`] that always resolves to the same [`BaseUri`].
//! - [`Router::fallback`] - creates a [`Router`] that selects between a primary and a fallback [`BaseUri`]
//!   based on the previous attempt's recovery information and the current
//!   attempt's position in the retry sequence.
//! - [`Router::custom`] - creates a [`Router`] that delegates the decision to a user supplied closure that
//!   receives a [`RouterContext`].
//!
//! # Conflict resolution
//!
//! When the target [`Uri`] passed to [`Router::resolve_uri`] already carries a
//! [`BaseUri`] and the routing also produces one, [`Router`] uses the configured
//! [`BaseUriConflict`] policy to decide what to do. The policy can be set with
//! [`Router::conflict_policy`] and defaults to [`BaseUriConflict::UseOriginal`].
//!
//! # Retry context
//!
//! A [`Router`] does not retry on its own; it only resolves a [`BaseUri`] for
//! a single attempt. When the routing decision should depend on previous
//! attempts, the caller passes that state in through a [`RouterContext`]
//! (attempt index, last-attempt flag, and the previous attempt's
//! [`RecoveryInfo`](recoverable::RecoveryInfo)).
//!
//! Populating the [`RouterContext`] is the caller's responsibility, typically
//! a resilience layer (e.g. a retry policy) wrapping the HTTP client: before
//! each attempt it builds a fresh context and calls
//! [`Router::resolve_request_uri`]. When no retry layer is involved,
//! [`RouterContext::new`] is sufficient.
//!
//! [`RecoveryInfo`](recoverable::RecoveryInfo) is the recoverable-error
//! metadata attached to the previous attempt's failure. In particular,
//! [`RecoveryKind::Unavailable`](recoverable::RecoveryKind::Unavailable)
//! signals that the endpoint just used is not serving traffic (e.g., an open
//! circuit breaker or a `503 Service Unavailable`). [`Router::fallback`]
//! keys off of this signal to swap endpoints on the next attempt.
//!
//! [`BaseUri`]: [template_uri::BaseUri]
//! [`Uri`]: [template_uri::Uri]

mod router;
pub use router::{BaseUriConflict, Router};

mod router_context;
pub use router_context::RouterContext;
