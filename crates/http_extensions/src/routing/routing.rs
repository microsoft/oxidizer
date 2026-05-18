// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;

use recoverable::RecoveryKind;
use templated_uri::{BaseUri, Uri};

use super::RoutingContext;
use crate::error_labels::{LABEL_URI_CONFLICT, LABEL_URI_MISSING};
use crate::{HttpError, HttpRequest};

/// Strategy used by [`Routing::create_uri`] when both the target [`Uri`] and the
/// routing produce a [`BaseUri`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum BaseUriConflict {
    /// Keep the [`BaseUri`] already present on the target [`Uri`] (the default).
    #[default]
    KeepExisting,

    /// Return an error when the target [`Uri`] already has a [`BaseUri`] and the
    /// routing also produces a conflicting [`BaseUri`].
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
/// let routing = Routing::base_uri(BaseUri::from_static("https://api.example.com"));
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
/// let routing = Routing::custom(|_ctx| Some(BaseUri::from_static("https://api.example.com")));
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
    resolver: Arc<Resolver>,
    conflict_policy: BaseUriConflict,
}

impl Routing {
    /// Creates a [`Routing`] that always returns the given [`BaseUri`].
    #[must_use]
    pub fn base_uri(base_uri: BaseUri) -> Self {
        Self {
            resolver: Arc::new(Resolver::Fixed(base_uri)),
            conflict_policy: BaseUriConflict::default(),
        }
    }

    /// Creates a [`Routing`] that selects between a primary and a fallback [`BaseUri`]
    /// based on the previous attempt's [`RecoveryInfo`] and the current attempt's
    /// position in the retry sequence.
    ///
    /// The first attempt always uses the primary [`BaseUri`]. On subsequent
    /// attempts, the fallback [`BaseUri`] is used when either:
    ///
    /// - the previous attempt's [`RecoveryInfo`] reports
    ///   [`RecoveryKind::Unavailable`] (e.g., a circuit breaker is open), or
    /// - the current attempt is the last attempt that will be performed
    ///   (a final best-effort try against the fallback endpoint).
    ///
    /// Otherwise, the primary [`BaseUri`] is used. This is intended for
    /// scenarios where the primary endpoint becomes unavailable but requests
    /// can still be served by a fallback endpoint.
    ///
    /// Uses [`BaseUriConflict::Override`] so in-place retries via
    /// [`Routing::update_request_uri`] can actually swap endpoints instead of
    /// being pinned to the primary [`BaseUri`] attached on the first attempt.
    ///
    /// [`RecoveryInfo`]: recoverable::RecoveryInfo
    /// [`RecoveryKind::Unavailable`]: recoverable::RecoveryKind::Unavailable
    #[must_use]
    pub fn fallback(primary: BaseUri, fallback: BaseUri) -> Self {
        Self::custom(move |ctx| Some(if use_fallback(ctx) { fallback.clone() } else { primary.clone() }))
            .conflict_policy(BaseUriConflict::Override)
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
            resolver: Arc::new(Resolver::Custom(Arc::new(resolver))),
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

    /// Returns `true` when this [`Routing`] may resolve to more than one
    /// [`BaseUri`] across attempts.
    ///
    /// Resilience layers can use this to decide whether retrying a request that
    /// previously failed with an unavailable endpoint is worthwhile: if
    /// alternatives exist, a subsequent attempt may be routed to a different
    /// endpoint and succeed.
    ///
    /// Returns `true` for [`Routing::fallback`] and [`Routing::custom`] (which
    /// may dynamically select among multiple endpoints), and `false` for
    /// [`Routing::base_uri`] and [`Routing::default`] (which always resolve to
    /// the same [`BaseUri`], or none at all).
    #[must_use]
    pub fn has_alternatives(&self) -> bool {
        matches!(self.resolver.as_ref(), Resolver::Custom(_))
    }

    /// Builds the final [`Uri`] for an outgoing request, combining the target [`Uri`] with
    /// the [`BaseUri`] produced by this routing according to the configured
    /// [`BaseUriConflict`] policy.
    ///
    /// # Errors
    ///
    /// Returns [`HttpError::validation`] when the policy is [`BaseUriConflict::Fail`] and
    /// either:
    /// - the target [`Uri`] already has a [`BaseUri`] and the routing also produces one; or
    /// - the target [`Uri`] has neither a [`BaseUri`] nor a path.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "while not consuming the context, we might do it at some point"
    )]
    pub fn create_uri(&self, ctx: RoutingContext, uri: Uri) -> Result<Uri, HttpError> {
        let (existing, path) = uri.into_parts();

        if existing.is_none() && path.is_none() && self.conflict_policy == BaseUriConflict::Fail {
            return Err(HttpError::validation_with_label(
                "the target URI cannot be empty; provide a base URI or a path such as `/...`",
                LABEL_URI_MISSING,
            ));
        }

        let routed = self.resolve(&ctx);

        // if new base uri is not available, return existing uri
        let Some(routed) = routed else {
            return Ok(Uri::from_parts(existing, path));
        };

        // if existing base uri is not available, return new base uri
        let Some(existing) = existing else {
            return Ok(Uri::from_parts(routed, path));
        };

        // choose base uri based on conflict policy
        let chosen = match self.conflict_policy {
            BaseUriConflict::KeepExisting => existing,
            BaseUriConflict::Override => routed,
            BaseUriConflict::Fail => {
                return Err(HttpError::validation_with_label(
                    "target URI already has a base URI; routing produced a conflicting base URI",
                    LABEL_URI_CONFLICT,
                ));
            }
        };

        Ok(Uri::from_parts(chosen, path))
    }

    /// Updates the [`HttpRequest`]'s URI in place by routing it through
    /// [`Routing::create_uri`].
    ///
    /// When `HttpRequestBuilder::build` attached the original templated [`Uri`]
    /// as a request extension, that non-routed target is used as the input on
    /// every call. This keeps repeated re-routings idempotent, e.g. fallback
    /// retries with [`BaseUriConflict::Override`] swap the [`BaseUri`] cleanly
    /// instead of stacking base path prefixes. Otherwise, the request's
    /// current [`http::Uri`] is used as the input.
    ///
    /// # Errors
    ///
    /// Returns [`HttpError::validation`] when:
    ///
    /// - the request's existing URI cannot be converted to a [`Uri`],
    /// - [`Routing::create_uri`] fails (e.g., a [`BaseUriConflict::Fail`] conflict), or
    /// - the resolved [`Uri`] cannot be converted back to an [`http::Uri`].
    pub fn update_request_uri(&self, ctx: RoutingContext, request: &mut HttpRequest) -> Result<(), HttpError> {
        // Prefer the original `Uri` extension so retries re-route from the
        // caller-supplied target; fall back to the request's current URI for
        // hand-built requests. Clone rather than take, so a failure below
        // leaves the request's URI untouched.
        let uri: Uri = match request.extensions().get::<Uri>() {
            Some(original) => original.clone(),
            None => request.uri().clone().try_into()?,
        };
        let resolved = self.create_uri(ctx.with_request(request), uri)?;
        *request.uri_mut() = resolved.try_into()?;
        Ok(())
    }

    /// Resolves the [`BaseUri`] for the current request, if any.
    fn resolve(&self, ctx: &RoutingContext) -> Option<BaseUri> {
        match self.resolver.as_ref() {
            Resolver::Empty => None,
            Resolver::Fixed(base_uri) => Some(base_uri.clone()),
            Resolver::Custom(f) => f(ctx),
        }
    }
}

// --- Private items below ---

type RoutingFn = dyn Fn(&RoutingContext) -> Option<BaseUri> + Send + Sync + 'static;

#[derive(Default)]
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

fn use_fallback(ctx: &RoutingContext) -> bool {
    // first attempt never uses fallback
    if ctx.attempt() == 0 {
        return false;
    }

    // best effort, for last attempt always try the fallback
    if ctx.is_last_attempt() {
        return true;
    }

    // use fallback if previous recovery was unavailable, this means that
    // last attempt that used primary endpoint reached unavailable endpoint
    ctx.previous_recovery().is_some_and(|info| info.kind() == RecoveryKind::Unavailable)
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
        let routing = Routing::base_uri(BaseUri::from_static("https://api.example.com"));
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
        let routing = Routing::custom(|_| Some(BaseUri::from_static("https://api.example.com")));
        let resolved = routing.create_uri(RoutingContext::new(), target_without_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://api.example.com/v1/items");
    }

    #[test]
    fn keep_existing_is_default_on_conflict() {
        let routing = Routing::base_uri(BaseUri::from_static("https://api.example.com"));

        let resolved = routing.create_uri(RoutingContext::new(), target_with_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://existing.example.com/items");
    }

    #[test]
    fn override_replaces_existing_base_uri() {
        let routing = Routing::base_uri(BaseUri::from_static("https://api.example.com")).conflict_policy(BaseUriConflict::Override);

        let resolved = routing.create_uri(RoutingContext::new(), target_with_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://api.example.com/items");
    }

    #[test]
    fn fail_returns_error_on_conflict() {
        let routing = Routing::base_uri(BaseUri::from_static("https://api.example.com")).conflict_policy(BaseUriConflict::Fail);

        let err = routing.create_uri(RoutingContext::new(), target_with_base()).unwrap_err();
        assert_eq!(err.label(), "uri_conflict");
    }

    #[test]
    fn fail_returns_error_when_target_has_no_base_and_no_path() {
        let routing = Routing::default().conflict_policy(BaseUriConflict::Fail);

        let err = routing.create_uri(RoutingContext::new(), Uri::default()).unwrap_err();
        assert_eq!(err.label(), "uri_missing");
    }

    #[test]
    fn fail_does_not_trigger_without_conflict() {
        let routing = Routing::base_uri(BaseUri::from_static("https://api.example.com")).conflict_policy(BaseUriConflict::Fail);

        let resolved = routing.create_uri(RoutingContext::new(), target_without_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://api.example.com/v1/items");
    }

    #[test]
    fn fallback_uses_primary_without_previous_recovery() {
        let routing = Routing::fallback(
            BaseUri::from_static("https://primary.example.com"),
            BaseUri::from_static("https://fallback.example.com"),
        );
        let resolved = routing.create_uri(RoutingContext::new(), target_without_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://primary.example.com/v1/items");
    }

    #[test]
    fn fallback_uses_primary_when_previous_recovery_is_not_unavailable() {
        let routing = Routing::fallback(
            BaseUri::from_static("https://primary.example.com"),
            BaseUri::from_static("https://fallback.example.com"),
        );
        let ctx = RoutingContext::new().with_previous_recovery(recoverable::RecoveryInfo::retry());
        let resolved = routing.create_uri(ctx, target_without_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://primary.example.com/v1/items");
    }

    #[test]
    fn fallback_uses_fallback_when_previous_recovery_is_unavailable() {
        let routing = Routing::fallback(
            BaseUri::from_static("https://primary.example.com"),
            BaseUri::from_static("https://fallback.example.com"),
        );
        let ctx = RoutingContext::new()
            .with_previous_recovery(recoverable::RecoveryInfo::unavailable())
            .with_attempt(1, false);
        let resolved = routing.create_uri(ctx, target_without_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://fallback.example.com/v1/items");
    }

    #[test]
    fn fallback_uses_fallback_on_last_attempt_after_first() {
        let routing = Routing::fallback(
            BaseUri::from_static("https://primary.example.com"),
            BaseUri::from_static("https://fallback.example.com"),
        );
        let ctx = RoutingContext::new().with_attempt(2, true);
        let resolved = routing.create_uri(ctx, target_without_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://fallback.example.com/v1/items");
    }

    #[test]
    fn fallback_uses_primary_on_first_attempt_even_when_last() {
        let routing = Routing::fallback(
            BaseUri::from_static("https://primary.example.com"),
            BaseUri::from_static("https://fallback.example.com"),
        );
        let ctx = RoutingContext::new().with_attempt(0, true);
        let resolved = routing.create_uri(ctx, target_without_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://primary.example.com/v1/items");
    }

    #[test]
    fn fallback_switches_endpoint_across_in_place_update_request_uri_calls() {
        // Regression test: ensure fallback retries can actually swap base URIs
        // when the request is re-routed in place across attempts. With the
        // default KeepExisting policy this would pin the request to the primary
        // forever after the first attempt.
        let routing = Routing::fallback(
            BaseUri::from_static("https://primary.example.com"),
            BaseUri::from_static("https://fallback.example.com"),
        );
        let mut request = crate::HttpRequestBuilder::new_fake().get("/v1/items").build().unwrap();

        // First attempt: primary endpoint.
        routing.update_request_uri(RoutingContext::new(), &mut request).unwrap();
        assert_eq!(request.uri().to_string(), "https://primary.example.com/v1/items");

        // Second attempt: previous recovery reports unavailable, fallback wins
        // and must replace the previously-attached primary base URI.
        let ctx = RoutingContext::new()
            .with_previous_recovery(recoverable::RecoveryInfo::unavailable())
            .with_attempt(1, false);
        routing.update_request_uri(ctx, &mut request).unwrap();
        assert_eq!(request.uri().to_string(), "https://fallback.example.com/v1/items");
    }

    #[test]
    fn fallback_does_not_duplicate_base_path_across_in_place_update_request_uri_calls() {
        // Regression test: when both endpoints carry a non-trivial base path,
        // re-routing in place must operate on the caller's original target.
        // Otherwise a fallback override would stack `/api/` on top of the
        // previously-routed `/api/v1/items`, yielding `/api/api/v1/items`.
        let routing = Routing::fallback(
            BaseUri::from_static("https://primary.example.com/api/v1/"),
            BaseUri::from_static("https://fallback.example.com/api/"),
        );
        let mut request = crate::HttpRequestBuilder::new_fake().get("/items").build().unwrap();

        // First attempt: primary endpoint.
        routing.update_request_uri(RoutingContext::new(), &mut request).unwrap();
        assert_eq!(request.uri().to_string(), "https://primary.example.com/api/v1/items");

        // Second attempt: fallback endpoint, not stacked on the previously-routed path.
        let ctx = RoutingContext::new()
            .with_previous_recovery(recoverable::RecoveryInfo::unavailable())
            .with_attempt(1, false);
        routing.update_request_uri(ctx, &mut request).unwrap();
        assert_eq!(request.uri().to_string(), "https://fallback.example.com/api/items");

        // Third attempt: back to primary, still clean.
        let ctx = RoutingContext::new().with_attempt(2, false);
        routing.update_request_uri(ctx, &mut request).unwrap();
        assert_eq!(request.uri().to_string(), "https://primary.example.com/api/v1/items");
    }

    #[test]
    fn fallback_uses_primary_on_non_last_attempt() {
        let routing = Routing::fallback(
            BaseUri::from_static("https://primary.example.com"),
            BaseUri::from_static("https://fallback.example.com"),
        );
        let ctx = RoutingContext::new().with_attempt(1, false);
        let resolved = routing.create_uri(ctx, target_without_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://primary.example.com/v1/items");
    }

    #[test]
    fn assert_routing_size() {
        static_assertions::assert_eq_size!(Routing, [u8; 16]);
    }

    #[test]
    fn default_has_no_alternatives() {
        assert!(!Routing::default().has_alternatives());
    }

    #[test]
    fn base_uri_has_no_alternatives() {
        let routing = Routing::base_uri(BaseUri::from_static("https://api.example.com"));
        assert!(!routing.has_alternatives());
    }

    #[test]
    fn fallback_has_alternatives() {
        let routing = Routing::fallback(
            BaseUri::from_static("https://primary.example.com"),
            BaseUri::from_static("https://fallback.example.com"),
        );
        assert!(routing.has_alternatives());
    }

    #[test]
    fn custom_has_alternatives() {
        let routing = Routing::custom(|_| None);
        assert!(routing.has_alternatives());
    }

    #[test]
    fn update_request_uri_attaches_base_uri() {
        let routing = Routing::base_uri(BaseUri::from_static("https://api.example.com"));
        let mut request = crate::HttpRequestBuilder::new_fake().get("/v1/items").build().unwrap();

        routing.update_request_uri(RoutingContext::new(), &mut request).unwrap();

        assert_eq!(request.uri().to_string(), "https://api.example.com/v1/items");
    }

    #[test]
    fn update_request_uri_keeps_existing_base_uri_by_default() {
        let routing = Routing::base_uri(BaseUri::from_static("https://api.example.com"));
        let mut request = crate::HttpRequestBuilder::new_fake()
            .get("https://existing.example.com/items")
            .build()
            .unwrap();

        routing.update_request_uri(RoutingContext::new(), &mut request).unwrap();

        assert_eq!(request.uri().to_string(), "https://existing.example.com/items");
    }

    #[test]
    fn update_request_uri_returns_error_on_conflict_when_policy_is_fail() {
        let routing = Routing::base_uri(BaseUri::from_static("https://api.example.com")).conflict_policy(BaseUriConflict::Fail);
        let mut request = crate::HttpRequestBuilder::new_fake()
            .get("https://existing.example.com/items")
            .build()
            .unwrap();

        let err = routing.update_request_uri(RoutingContext::new(), &mut request).unwrap_err();
        assert_eq!(err.label(), "uri_conflict");
    }

    #[test]
    fn update_request_uri_preserves_original_uri_on_failure() {
        let routing = Routing::base_uri(BaseUri::from_static("https://api.example.com")).conflict_policy(BaseUriConflict::Fail);
        let mut request = crate::HttpRequestBuilder::new_fake()
            .get("https://existing.example.com/items")
            .build()
            .unwrap();

        let original_uri = request.uri().clone();
        let _ = routing.update_request_uri(RoutingContext::new(), &mut request).unwrap_err();

        assert_eq!(request.uri(), &original_uri);
    }

    #[test]
    fn resolver_debug_format() {
        assert_eq!(format!("{:?}", Resolver::Empty), "Empty");

        let fixed = Resolver::Fixed(BaseUri::from_static("https://api.example.com"));
        assert!(format!("{fixed:?}").starts_with("Fixed("));

        let custom = Resolver::Custom(Arc::new(|_| None));
        assert_eq!(format!("{custom:?}"), "Custom");
    }
}
