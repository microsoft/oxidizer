// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;

use recoverable::RecoveryKind;
use templated_uri::{BaseUri, Uri};

use super::RouterContext;
use crate::error_labels::{LABEL_URI_CONFLICT, LABEL_URI_MISSING};
use crate::{HttpError, HttpRequest};

/// Strategy used by [`Router::create_uri`] when both the target [`Uri`] and the
/// routing produce a [`BaseUri`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum BaseUriConflict {
    /// Use the original [`BaseUri`] already present on the target [`Uri`] and
    /// discard the one produced by the routing (the default).
    #[default]
    UseOriginal,

    /// Use the [`BaseUri`] produced by the routing and discard the original one
    /// already present on the target [`Uri`].
    UseRouted,

    /// Return an error when the target [`Uri`] already has a [`BaseUri`] and the
    /// routing also produces a conflicting [`BaseUri`].
    Fail,
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
/// use http_extensions::routing::{Router, RouterContext};
/// use templated_uri::{BaseUri, Uri};
///
/// let router = Router::fixed(BaseUri::from_static("https://api.example.com"));
/// let target: Uri = "/v1/items".parse().unwrap();
///
/// let resolved = router.create_uri(RouterContext::new(), target).unwrap();
/// assert_eq!(
///     resolved.to_string().declassify_into(),
///     "https://api.example.com/v1/items"
/// );
/// ```
///
/// Pick a [`BaseUri`] dynamically:
///
/// ```
/// use http_extensions::routing::{Router, RouterContext};
/// use templated_uri::{BaseUri, Uri};
///
/// let router = Router::custom(|context| Some(BaseUri::from_static("https://api.example.com")));
/// let target: Uri = "/v1/items".parse().unwrap();
///
/// let resolved = router.create_uri(RouterContext::new(), target).unwrap();
/// assert_eq!(
///     resolved.to_string().declassify_into(),
///     "https://api.example.com/v1/items"
/// );
/// ```
#[derive(Debug, Clone, Default)]
pub struct Router {
    resolver: Arc<Resolver>,
    conflict_policy: BaseUriConflict,
}

impl Router {
    /// Creates a [`Router`] that always resolves to the given [`BaseUri`].
    #[must_use]
    pub fn fixed(base_uri: BaseUri) -> Self {
        Self {
            resolver: Arc::new(Resolver::Fixed(base_uri)),
            conflict_policy: BaseUriConflict::default(),
        }
    }

    /// Creates a [`Router`] that selects between a primary and a fallback [`BaseUri`]
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
    /// Uses [`BaseUriConflict::UseRouted`] so in-place retries via
    /// [`Router::update_request_uri`] can actually swap endpoints instead of
    /// being pinned to the primary [`BaseUri`] attached on the first attempt.
    ///
    /// [`RecoveryInfo`]: recoverable::RecoveryInfo
    /// [`RecoveryKind::Unavailable`]: recoverable::RecoveryKind::Unavailable
    #[must_use]
    pub fn fallback(primary: BaseUri, fallback: BaseUri) -> Self {
        Self::custom(move |context| Some(if use_fallback(context) { fallback.clone() } else { primary.clone() }))
            .conflict_policy(BaseUriConflict::UseRouted)
    }

    /// Creates a [`Router`] that delegates resolution to the given closure.
    ///
    /// The closure receives a [`RouterContext`] and returns `Some(BaseUri)` to attach a
    /// [`BaseUri`] to the target, or `None` to leave the target's [`BaseUri`] as-is.
    #[must_use]
    pub fn custom<F>(resolver: F) -> Self
    where
        F: Fn(&RouterContext) -> Option<BaseUri> + Send + Sync + 'static,
    {
        Self {
            resolver: Arc::new(Resolver::Custom(Arc::new(resolver))),
            conflict_policy: BaseUriConflict::default(),
        }
    }

    /// Sets the [`BaseUriConflict`] policy used by [`Router::create_uri`] when both the
    /// target [`Uri`] and the routing produce a [`BaseUri`].
    #[must_use]
    pub fn conflict_policy(mut self, policy: BaseUriConflict) -> Self {
        self.conflict_policy = policy;
        self
    }

    /// Returns `true` when this [`Router`] may resolve to more than one
    /// [`BaseUri`] across attempts.
    ///
    /// Resilience layers can use this to decide whether retrying a request that
    /// previously failed with an unavailable endpoint is worthwhile: if
    /// alternatives exist, a subsequent attempt may be routed to a different
    /// endpoint and succeed.
    ///
    /// Returns `true` for [`Router::fallback`] and [`Router::custom`] (which
    /// may dynamically select among multiple endpoints), and `false` for
    /// [`Router::fixed`] and [`Router::default`] (which always resolve to
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
    pub fn create_uri(&self, context: RouterContext, uri: Uri) -> Result<Uri, HttpError> {
        let (existing, path) = uri.into_parts();

        if existing.is_none() && path.is_none() && self.conflict_policy == BaseUriConflict::Fail {
            return Err(HttpError::validation_with_label(
                "the target URI cannot be empty; provide a base URI or a path such as `/...`",
                LABEL_URI_MISSING,
            ));
        }

        let routed = self.resolve(&context);

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
            BaseUriConflict::UseOriginal => existing,
            BaseUriConflict::UseRouted => routed,
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
    /// [`Router::create_uri`].
    ///
    /// When `HttpRequestBuilder::build` attached the original templated [`Uri`]
    /// as a request extension, that non-routed target is used as the input on
    /// every call. This keeps repeated re-routings idempotent, e.g. fallback
    /// retries with [`BaseUriConflict::UseRouted`] swap the [`BaseUri`] cleanly
    /// instead of stacking base path prefixes. Otherwise, the request's
    /// current [`http::Uri`] is used as the input.
    ///
    /// # Errors
    ///
    /// Returns [`HttpError::validation`] when:
    ///
    /// - the request's existing URI cannot be converted to a [`Uri`],
    /// - [`Router::create_uri`] fails (e.g., a [`BaseUriConflict::Fail`] conflict), or
    /// - the resolved [`Uri`] cannot be converted back to an [`http::Uri`].
    pub fn update_request_uri(&self, context: RouterContext, request: &mut HttpRequest) -> Result<(), HttpError> {
        // Prefer the original `Uri` extension so retries re-route from the
        // caller-supplied target; fall back to the request's current URI for
        // hand-built requests. Clone rather than take, so a failure below
        // leaves the request's URI untouched.
        let uri: Uri = match request.extensions().get::<Uri>() {
            Some(original) => original.clone(),
            None => request.uri().clone().try_into()?,
        };
        let resolved = self.create_uri(context.with_request(request), uri)?;
        *request.uri_mut() = resolved.try_into()?;
        Ok(())
    }

    /// Resolves the [`BaseUri`] for the current request, if any.
    fn resolve(&self, context: &RouterContext) -> Option<BaseUri> {
        match self.resolver.as_ref() {
            Resolver::Empty => None,
            Resolver::Fixed(base_uri) => Some(base_uri.clone()),
            Resolver::Custom(f) => f(context),
        }
    }
}

// --- Private items below ---

type RouterFn = dyn Fn(&RouterContext) -> Option<BaseUri> + Send + Sync + 'static;

#[derive(Default)]
enum Resolver {
    #[default]
    Empty,
    Fixed(BaseUri),
    Custom(Arc<RouterFn>),
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

fn use_fallback(context: &RouterContext) -> bool {
    // first attempt never uses fallback
    if context.attempt() == 0 {
        return false;
    }

    // best effort, for last attempt always try the fallback
    if context.is_last_attempt() {
        return true;
    }

    // use fallback if previous recovery was unavailable, this means that
    // last attempt that used primary endpoint reached unavailable endpoint
    context
        .previous_recovery()
        .is_some_and(|info| info.kind() == RecoveryKind::Unavailable)
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
        let router = Router::default();

        let with_base = router.create_uri(RouterContext::new(), target_with_base()).unwrap();
        assert_eq!(with_base.to_string().declassify_into(), "https://existing.example.com/items");

        let without_base = router.create_uri(RouterContext::new(), target_without_base()).unwrap();
        assert_eq!(without_base.to_string().declassify_into(), "/v1/items");
    }

    #[test]
    fn fixed_attaches_when_target_has_none() {
        let router = Router::fixed(BaseUri::from_static("https://api.example.com"));
        let resolved = router.create_uri(RouterContext::new(), target_without_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://api.example.com/v1/items");
    }

    #[test]
    fn custom_resolver_returning_none_passes_through() {
        let router = Router::custom(|_| None);
        let resolved = router.create_uri(RouterContext::new(), target_with_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://existing.example.com/items");
    }

    #[test]
    fn custom_resolver_returning_some_is_used() {
        let router = Router::custom(|_| Some(BaseUri::from_static("https://api.example.com")));
        let resolved = router.create_uri(RouterContext::new(), target_without_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://api.example.com/v1/items");
    }

    #[test]
    fn keep_existing_is_default_on_conflict() {
        let router = Router::fixed(BaseUri::from_static("https://api.example.com"));

        let resolved = router.create_uri(RouterContext::new(), target_with_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://existing.example.com/items");
    }

    #[test]
    fn use_routed_replaces_original_base_uri() {
        let router = Router::fixed(BaseUri::from_static("https://api.example.com")).conflict_policy(BaseUriConflict::UseRouted);

        let resolved = router.create_uri(RouterContext::new(), target_with_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://api.example.com/items");
    }

    #[test]
    fn fail_returns_error_on_conflict() {
        let router = Router::fixed(BaseUri::from_static("https://api.example.com")).conflict_policy(BaseUriConflict::Fail);

        let err = router.create_uri(RouterContext::new(), target_with_base()).unwrap_err();
        assert_eq!(err.label(), "uri_conflict");
    }

    #[test]
    fn fail_returns_error_when_target_has_no_base_and_no_path() {
        let router = Router::default().conflict_policy(BaseUriConflict::Fail);

        let err = router.create_uri(RouterContext::new(), Uri::default()).unwrap_err();
        assert_eq!(err.label(), "uri_missing");
    }

    #[test]
    fn fail_does_not_trigger_without_conflict() {
        let router = Router::fixed(BaseUri::from_static("https://api.example.com")).conflict_policy(BaseUriConflict::Fail);

        let resolved = router.create_uri(RouterContext::new(), target_without_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://api.example.com/v1/items");
    }

    #[test]
    fn fallback_uses_primary_without_previous_recovery() {
        let router = Router::fallback(
            BaseUri::from_static("https://primary.example.com"),
            BaseUri::from_static("https://fallback.example.com"),
        );
        let resolved = router.create_uri(RouterContext::new(), target_without_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://primary.example.com/v1/items");
    }

    #[test]
    fn fallback_uses_primary_when_previous_recovery_is_not_unavailable() {
        let router = Router::fallback(
            BaseUri::from_static("https://primary.example.com"),
            BaseUri::from_static("https://fallback.example.com"),
        );
        let ctx = RouterContext::new().with_previous_recovery(recoverable::RecoveryInfo::retry());
        let resolved = router.create_uri(ctx, target_without_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://primary.example.com/v1/items");
    }

    #[test]
    fn fallback_uses_fallback_when_previous_recovery_is_unavailable() {
        let router = Router::fallback(
            BaseUri::from_static("https://primary.example.com"),
            BaseUri::from_static("https://fallback.example.com"),
        );
        let ctx = RouterContext::new()
            .with_previous_recovery(recoverable::RecoveryInfo::unavailable())
            .with_attempt(1, false);
        let resolved = router.create_uri(ctx, target_without_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://fallback.example.com/v1/items");
    }

    #[test]
    fn fallback_uses_fallback_on_last_attempt_after_first() {
        let router = Router::fallback(
            BaseUri::from_static("https://primary.example.com"),
            BaseUri::from_static("https://fallback.example.com"),
        );
        let ctx = RouterContext::new().with_attempt(2, true);
        let resolved = router.create_uri(ctx, target_without_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://fallback.example.com/v1/items");
    }

    #[test]
    fn fallback_uses_primary_on_first_attempt_even_when_last() {
        let router = Router::fallback(
            BaseUri::from_static("https://primary.example.com"),
            BaseUri::from_static("https://fallback.example.com"),
        );
        let ctx = RouterContext::new().with_attempt(0, true);
        let resolved = router.create_uri(ctx, target_without_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://primary.example.com/v1/items");
    }

    #[test]
    fn fallback_switches_endpoint_across_in_place_update_request_uri_calls() {
        // Regression test: ensure fallback retries can actually swap base URIs
        // when the request is re-routed in place across attempts. With the
        // default UseOriginal policy this would pin the request to the primary
        // forever after the first attempt.
        let router = Router::fallback(
            BaseUri::from_static("https://primary.example.com"),
            BaseUri::from_static("https://fallback.example.com"),
        );
        let mut request = crate::HttpRequestBuilder::new_fake().get("/v1/items").build().unwrap();

        // First attempt: primary endpoint.
        router.update_request_uri(RouterContext::new(), &mut request).unwrap();
        assert_eq!(request.uri().to_string(), "https://primary.example.com/v1/items");

        // Second attempt: previous recovery reports unavailable, fallback wins
        // and must replace the previously-attached primary base URI on the request.
        let ctx = RouterContext::new()
            .with_previous_recovery(recoverable::RecoveryInfo::unavailable())
            .with_attempt(1, false);
        router.update_request_uri(ctx, &mut request).unwrap();
        assert_eq!(request.uri().to_string(), "https://fallback.example.com/v1/items");
    }

    #[test]
    fn fallback_does_not_duplicate_base_path_across_in_place_update_request_uri_calls() {
        // Regression test: when both endpoints carry a non-trivial base path,
        // re-routing in place must operate on the caller's original target.
        // Otherwise a fallback re-route would stack `/api/` on top of the
        // previously-routed `/api/v1/items`, yielding `/api/api/v1/items`.
        let router = Router::fallback(
            BaseUri::from_static("https://primary.example.com/api/v1/"),
            BaseUri::from_static("https://fallback.example.com/api/"),
        );
        let mut request = crate::HttpRequestBuilder::new_fake().get("/items").build().unwrap();

        // First attempt: primary endpoint.
        router.update_request_uri(RouterContext::new(), &mut request).unwrap();
        assert_eq!(request.uri().to_string(), "https://primary.example.com/api/v1/items");

        // Second attempt: fallback endpoint, not stacked on the previously-routed path.
        let ctx = RouterContext::new()
            .with_previous_recovery(recoverable::RecoveryInfo::unavailable())
            .with_attempt(1, false);
        router.update_request_uri(ctx, &mut request).unwrap();
        assert_eq!(request.uri().to_string(), "https://fallback.example.com/api/items");

        // Third attempt: back to primary, still clean.
        let ctx = RouterContext::new().with_attempt(2, false);
        router.update_request_uri(ctx, &mut request).unwrap();
        assert_eq!(request.uri().to_string(), "https://primary.example.com/api/v1/items");
    }

    #[test]
    fn fallback_uses_primary_on_non_last_attempt() {
        let router = Router::fallback(
            BaseUri::from_static("https://primary.example.com"),
            BaseUri::from_static("https://fallback.example.com"),
        );
        let ctx = RouterContext::new().with_attempt(1, false);
        let resolved = router.create_uri(ctx, target_without_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://primary.example.com/v1/items");
    }

    #[test]
    fn assert_router_size() {
        static_assertions::assert_eq_size!(Router, [u8; 16]);
    }

    #[test]
    fn default_has_no_alternatives() {
        assert!(!Router::default().has_alternatives());
    }

    #[test]
    fn fixed_has_no_alternatives() {
        let router = Router::fixed(BaseUri::from_static("https://api.example.com"));
        assert!(!router.has_alternatives());
    }

    #[test]
    fn fallback_has_alternatives() {
        let router = Router::fallback(
            BaseUri::from_static("https://primary.example.com"),
            BaseUri::from_static("https://fallback.example.com"),
        );
        assert!(router.has_alternatives());
    }

    #[test]
    fn custom_has_alternatives() {
        let router = Router::custom(|_| None);
        assert!(router.has_alternatives());
    }

    #[test]
    fn update_request_uri_falls_back_to_request_uri_without_uri_extension() {
        // Hand-built requests (constructed directly via `http::Request::new`)
        // do not carry the templated `Uri` extension that
        // `HttpRequestBuilder::build` attaches. In that case
        // `update_request_uri` must fall back to converting the request's
        // current `http::Uri` and route from that.
        let router = Router::fixed(BaseUri::from_static("https://api.example.com"));
        let body = crate::HttpBodyBuilder::new_fake().empty();
        let mut request = http::Request::new(body);
        *request.uri_mut() = http::Uri::from_static("/v1/items");
        assert!(request.extensions().get::<Uri>().is_none(), "precondition: no Uri extension");

        router.update_request_uri(RouterContext::new(), &mut request).unwrap();

        assert_eq!(request.uri().to_string(), "https://api.example.com/v1/items");
    }

    #[test]
    fn update_request_uri_attaches_base_uri() {
        let router = Router::fixed(BaseUri::from_static("https://api.example.com"));
        let mut request = crate::HttpRequestBuilder::new_fake().get("/v1/items").build().unwrap();

        router.update_request_uri(RouterContext::new(), &mut request).unwrap();

        assert_eq!(request.uri().to_string(), "https://api.example.com/v1/items");
    }

    #[test]
    fn update_request_uri_keeps_existing_base_uri_by_default() {
        let router = Router::fixed(BaseUri::from_static("https://api.example.com"));
        let mut request = crate::HttpRequestBuilder::new_fake()
            .get("https://existing.example.com/items")
            .build()
            .unwrap();

        router.update_request_uri(RouterContext::new(), &mut request).unwrap();

        assert_eq!(request.uri().to_string(), "https://existing.example.com/items");
    }

    #[test]
    fn update_request_uri_returns_error_on_conflict_when_policy_is_fail() {
        let router = Router::fixed(BaseUri::from_static("https://api.example.com")).conflict_policy(BaseUriConflict::Fail);
        let mut request = crate::HttpRequestBuilder::new_fake()
            .get("https://existing.example.com/items")
            .build()
            .unwrap();

        let err = router.update_request_uri(RouterContext::new(), &mut request).unwrap_err();
        assert_eq!(err.label(), "uri_conflict");
    }

    #[test]
    fn update_request_uri_preserves_original_uri_on_failure() {
        let router = Router::fixed(BaseUri::from_static("https://api.example.com")).conflict_policy(BaseUriConflict::Fail);
        let mut request = crate::HttpRequestBuilder::new_fake()
            .get("https://existing.example.com/items")
            .build()
            .unwrap();

        let original_uri = request.uri().clone();
        let _ = router.update_request_uri(RouterContext::new(), &mut request).unwrap_err();

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
