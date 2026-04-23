// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;

use recoverable::RecoveryKind;
use templated_uri::{BaseUri, Uri};

use super::RoutingContext;
use crate::HttpError;
use crate::error_labels::LABEL_ROUTING_BASE_URI_CONFLICT;

/// Strategy used by [`Routing::create_uri`] when both the target [`Uri`] and the
/// routing produce a [`BaseUri`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum BaseUriConflict {
    /// Keep the [`BaseUri`] already present on the target [`Uri`] (the default).
    #[default]
    KeepExisting,

    /// Return an error when the target [`Uri`] already has a [`BaseUri`].
    Fail,

    /// Replace the target's [`BaseUri`] with the one produced by the routing.
    Override,
}

/// Resolves the [`BaseUri`] to use for an outgoing request.
///
/// See the [module documentation](super) for an overview.
///
/// # Examples
///
/// Always route to a single base URI:
///
/// ```
/// use http_extensions::routing::{Routing, RoutingContext};
/// use templated_uri::{BaseUri, Uri};
///
/// let routing = Routing::base_uri(BaseUri::from_uri_static("https://api.example.com"));
/// let target: Uri = "/v1/items".parse().unwrap();
///
/// let resolved = routing.create_uri(RoutingContext::new(), target).unwrap();
/// assert_eq!(
///     resolved.to_string().declassify_into(),
///     "https://api.example.com/v1/items"
/// );
/// ```
///
/// Pick a [`BaseUri`] dynamically:
///
/// ```
/// use http_extensions::routing::{Routing, RoutingContext};
/// use templated_uri::{BaseUri, Uri};
///
/// let routing = Routing::custom(|_ctx| Some(BaseUri::from_uri_static("https://api.example.com")));
/// let target: Uri = "/v1/items".parse().unwrap();
///
/// let resolved = routing.create_uri(RoutingContext::new(), target).unwrap();
/// assert_eq!(
///     resolved.to_string().declassify_into(),
///     "https://api.example.com/v1/items"
/// );
/// ```
#[derive(Debug, Clone, Default)]
pub struct Routing {
    resolver: Resolver,
    conflict_policy: BaseUriConflict,
}

impl Routing {
    /// Creates a [`Routing`] that always returns the given [`BaseUri`].
    #[must_use]
    pub fn base_uri(base_uri: BaseUri) -> Self {
        Self {
            resolver: Resolver::Fixed(base_uri),
            conflict_policy: BaseUriConflict::default(),
        }
    }

    /// Creates a [`Routing`] that selects between a primary and a fallback [`BaseUri`]
    /// based on the previous attempt's [`RecoveryInfo`].
    ///
    /// The primary [`BaseUri`] is used unless the previous attempt's
    /// [`RecoveryInfo`] reports [`RecoveryKind::Unavailable`], in which case the
    /// fallback [`BaseUri`] is used. This is intended for scenarios where the
    /// primary endpoint becomes unavailable (e.g., a circuit breaker is open) but
    /// requests can still be served by a fallback endpoint.
    ///
    /// [`RecoveryInfo`]: recoverable::RecoveryInfo
    /// [`RecoveryKind::Unavailable`]: recoverable::RecoveryKind::Unavailable
    #[must_use]
    pub fn fallback(primary: BaseUri, fallback: BaseUri) -> Self {
        Self::custom(move |ctx| {
            let use_fallback = ctx.previous_recovery().is_some_and(|info| info.kind() == RecoveryKind::Unavailable);
            Some(if use_fallback { fallback.clone() } else { primary.clone() })
        })
    }

    /// Creates a [`Routing`] that delegates resolution to the given closure.
    ///
    /// The closure receives a [`RoutingContext`] and returns `Some(BaseUri)` to attach a
    /// [`BaseUri`] to the target, or `None` to leave the target's [`BaseUri`] as-is.
    #[must_use]
    pub fn custom<F>(resolver: F) -> Self
    where
        F: Fn(&RoutingContext) -> Option<BaseUri> + Send + Sync + 'static,
    {
        Self {
            resolver: Resolver::Custom(Arc::new(resolver)),
            conflict_policy: BaseUriConflict::default(),
        }
    }

    /// Sets the [`BaseUriConflict`] policy used by [`Routing::create_uri`] when both the
    /// target [`Uri`] and the routing produce a [`BaseUri`].
    #[must_use]
    pub fn conflict_policy(mut self, policy: BaseUriConflict) -> Self {
        self.conflict_policy = policy;
        self
    }

    /// Builds the final [`Uri`] for an outgoing request, combining the target [`Uri`] with
    /// the [`BaseUri`] produced by this routing according to the configured
    /// [`BaseUriConflict`] policy.
    ///
    /// # Errors
    ///
    /// Returns [`HttpError::validation`] when the target [`Uri`] already has a [`BaseUri`],
    /// the routing also produces one, and the policy is [`BaseUriConflict::Fail`].
    pub fn create_uri(&self, ctx: RoutingContext, uri: Uri) -> Result<Uri, HttpError> {
        let routed = self.resolve(&ctx);
        let (existing, path) = uri.into_parts();

        // if new base uri is not available, return existing uri
        let Some(routed) = routed else {
            return Ok(Uri::with_base_and_path(existing, path));
        };

        // if existing base uri is not available, return new base uri
        let Some(existing) = existing else {
            return Ok(Uri::with_base_and_path(Some(routed), path));
        };

        // choose base uri based on conflict policy
        let chosen = match self.conflict_policy {
            BaseUriConflict::KeepExisting => existing,
            BaseUriConflict::Override => routed,
            BaseUriConflict::Fail => {
                return Err(HttpError::validation_with_label(
                    "target URI already has a base URI; routing produced a conflicting base URI",
                    LABEL_ROUTING_BASE_URI_CONFLICT,
                ));
            }
        };

        Ok(Uri::with_base_and_path(Some(chosen), path))
    }

    /// Resolves the [`BaseUri`] for the current request, if any.
    fn resolve(&self, ctx: &RoutingContext) -> Option<BaseUri> {
        match &self.resolver {
            Resolver::Empty => None,
            Resolver::Fixed(base_uri) => Some(base_uri.clone()),
            Resolver::Custom(f) => f(ctx),
        }
    }
}

// --- Private items below ---

type RoutingFn = dyn Fn(&RoutingContext) -> Option<BaseUri> + Send + Sync + 'static;

#[derive(Clone, Default)]
enum Resolver {
    #[default]
    Empty,
    Fixed(BaseUri),
    Custom(Arc<RoutingFn>),
}

impl std::fmt::Debug for Resolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("Empty"),
            Self::Fixed(base_uri) => f.debug_tuple("Fixed").field(base_uri).finish(),
            Self::Custom(_) => f.write_str("Custom"),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use ohno::Labeled;

    use super::*;

    fn target_with_base() -> Uri {
        "https://existing.example.com/items".parse().unwrap()
    }

    fn target_without_base() -> Uri {
        "/v1/items".parse().unwrap()
    }

    #[test]
    fn default_passes_target_through() {
        let routing = Routing::default();

        let with_base = routing.create_uri(RoutingContext::new(), target_with_base()).unwrap();
        assert_eq!(with_base.to_string().declassify_into(), "https://existing.example.com/items");

        let without_base = routing.create_uri(RoutingContext::new(), target_without_base()).unwrap();
        assert_eq!(without_base.to_string().declassify_into(), "/v1/items");
    }

    #[test]
    fn base_uri_attaches_when_target_has_none() {
        let routing = Routing::base_uri(BaseUri::from_uri_static("https://api.example.com"));
        let resolved = routing.create_uri(RoutingContext::new(), target_without_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://api.example.com/v1/items");
    }

    #[test]
    fn custom_resolver_returning_none_passes_through() {
        let routing = Routing::custom(|_| None);
        let resolved = routing.create_uri(RoutingContext::new(), target_with_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://existing.example.com/items");
    }

    #[test]
    fn custom_resolver_returning_some_is_used() {
        let routing = Routing::custom(|_| Some(BaseUri::from_uri_static("https://api.example.com")));
        let resolved = routing.create_uri(RoutingContext::new(), target_without_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://api.example.com/v1/items");
    }

    #[test]
    fn keep_existing_is_default_on_conflict() {
        let routing = Routing::base_uri(BaseUri::from_uri_static("https://api.example.com"));

        let resolved = routing.create_uri(RoutingContext::new(), target_with_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://existing.example.com/items");
    }

    #[test]
    fn override_replaces_existing_base_uri() {
        let routing = Routing::base_uri(BaseUri::from_uri_static("https://api.example.com")).conflict_policy(BaseUriConflict::Override);

        let resolved = routing.create_uri(RoutingContext::new(), target_with_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://api.example.com/items");
    }

    #[test]
    fn fail_returns_error_on_conflict() {
        let routing = Routing::base_uri(BaseUri::from_uri_static("https://api.example.com")).conflict_policy(BaseUriConflict::Fail);

        let err = routing.create_uri(RoutingContext::new(), target_with_base()).unwrap_err();
        assert_eq!(err.label(), "routing_base_uri_conflict");
    }

    #[test]
    fn fail_does_not_trigger_without_conflict() {
        let routing = Routing::base_uri(BaseUri::from_uri_static("https://api.example.com")).conflict_policy(BaseUriConflict::Fail);

        let resolved = routing.create_uri(RoutingContext::new(), target_without_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://api.example.com/v1/items");
    }

    #[test]
    fn fallback_uses_primary_without_previous_recovery() {
        let routing = Routing::fallback(
            BaseUri::from_uri_static("https://primary.example.com"),
            BaseUri::from_uri_static("https://fallback.example.com"),
        );
        let resolved = routing.create_uri(RoutingContext::new(), target_without_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://primary.example.com/v1/items");
    }

    #[test]
    fn fallback_uses_primary_when_previous_recovery_is_not_unavailable() {
        let routing = Routing::fallback(
            BaseUri::from_uri_static("https://primary.example.com"),
            BaseUri::from_uri_static("https://fallback.example.com"),
        );
        let ctx = RoutingContext::new().with_previous_recovery(recoverable::RecoveryInfo::retry());
        let resolved = routing.create_uri(ctx, target_without_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://primary.example.com/v1/items");
    }

    #[test]
    fn fallback_uses_fallback_when_previous_recovery_is_unavailable() {
        let routing = Routing::fallback(
            BaseUri::from_uri_static("https://primary.example.com"),
            BaseUri::from_uri_static("https://fallback.example.com"),
        );
        let ctx = RoutingContext::new().with_previous_recovery(recoverable::RecoveryInfo::unavailable());
        let resolved = routing.create_uri(ctx, target_without_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://fallback.example.com/v1/items");
    }
}
