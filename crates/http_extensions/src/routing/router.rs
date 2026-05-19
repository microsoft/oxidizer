// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;

use recoverable::RecoveryKind;
use templated_uri::{BaseUri, Uri};

use super::RouterContext;
use crate::error_labels::{LABEL_URI_CONFLICT, LABEL_URI_MISSING};
use crate::{HttpError, HttpRequest};

/// Strategy used by [`Router::resolve_uri`] when both the target [`Uri`] and the
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
/// let resolved = router.resolve_uri(RouterContext::new(), target).unwrap();
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
/// let router = Router::custom(
///     |context| Some(BaseUri::from_static("https://api.example.com")),
///     false,
/// );
/// let target: Uri = "/v1/items".parse().unwrap();
///
/// let resolved = router.resolve_uri(RouterContext::new(), target).unwrap();
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

/// Request extension carrying the caller's [`original`](Self::original)
/// templated [`Uri`] and the most recently [`routed`](Self::routed) result.
///
/// [`Router::resolve_request_uri`] always routes from [`original`](Self::original)
/// so repeated re-routings are idempotent, and overwrites only
/// [`routed`](Self::routed) on each call.
#[derive(Clone, Debug)]
pub struct RequestUris {
    original: Uri,
    routed: Option<Uri>,
}

impl RequestUris {
    /// Creates a [`RequestUris`] with the given templated [`Uri`] as the
    /// [`original`](Self::original) and no [`routed`](Self::routed) yet.
    #[must_use]
    pub fn new(original: Uri) -> Self {
        Self { original, routed: None }
    }

    /// Returns the caller-supplied templated [`Uri`].
    #[must_use]
    pub fn original(&self) -> &Uri {
        &self.original
    }

    /// Returns the most recently resolved [`Uri`], if any.
    #[must_use]
    pub fn routed(&self) -> Option<&Uri> {
        self.routed.as_ref()
    }

    /// Records the most recently resolved [`Uri`].
    pub fn set_routed(&mut self, routed: Uri) {
        self.routed = Some(routed);
    }
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
    /// based on the per-attempt information in the [`RouterContext`].
    ///
    /// The first attempt uses the primary [`BaseUri`]. Subsequent attempts use
    /// the fallback when either the previous attempt reported
    /// [`RecoveryKind::Unavailable`] (e.g., a circuit breaker is open), or the
    /// current attempt is the last one (a final best-effort try).
    ///
    /// Attempt index, last-attempt flag, and previous-attempt [`RecoveryInfo`][recoverable::RecoveryInfo]
    /// are populated by the caller driving the recovery loop; see the
    /// [module-level "Recovery Context" section][super#recovery-context].
    ///
    /// Uses [`BaseUriConflict::UseRouted`] so in-place recovery attempts via
    /// [`Router::resolve_request_uri`] can swap endpoints instead of being
    /// pinned to the primary [`BaseUri`] attached on the first attempt.
    #[must_use]
    pub fn fallback(primary: BaseUri, fallback: BaseUri) -> Self {
        Self::custom(
            move |context| Some(if use_fallback(context) { fallback.clone() } else { primary.clone() }),
            true,
        )
        .conflict_policy(BaseUriConflict::UseRouted)
    }

    /// Creates a [`Router`] that delegates resolution to the given closure.
    ///
    /// The closure receives a [`RouterContext`] and returns `Some(BaseUri)` to attach a
    /// [`BaseUri`] to the target, or `None` to leave the target's [`BaseUri`] as-is.
    ///
    /// `has_alternatives` declares whether the closure may select among
    /// multiple endpoints across attempts; it is reported by
    /// [`Router::has_alternatives`] and used by resilience layers to decide
    /// whether recovering from an `Unavailable` outcome is worthwhile.
    #[must_use]
    pub fn custom<F>(resolver: F, has_alternatives: bool) -> Self
    where
        F: Fn(&RouterContext) -> Option<BaseUri> + Send + Sync + 'static,
    {
        Self {
            resolver: Arc::new(Resolver::Custom {
                resolver: Arc::new(resolver),
                has_alternatives,
            }),
            conflict_policy: BaseUriConflict::default(),
        }
    }

    /// Sets the [`BaseUriConflict`] policy used by [`Router::resolve_uri`] when both the
    /// target [`Uri`] and the routing produce a [`BaseUri`].
    #[must_use]
    pub fn conflict_policy(mut self, policy: BaseUriConflict) -> Self {
        self.conflict_policy = policy;
        self
    }

    /// Returns `true` when this [`Router`] may resolve to more than one
    /// [`BaseUri`] across attempts.
    ///
    /// Resilience layers can use this to decide whether recovering a request that
    /// previously failed with an unavailable endpoint is worthwhile: if
    /// alternatives exist, a subsequent attempt may be routed to a different
    /// endpoint and succeed.
    #[must_use]
    pub fn has_alternatives(&self) -> bool {
        match self.resolver.as_ref() {
            Resolver::Empty | Resolver::Fixed(_) => false,
            Resolver::Custom { has_alternatives, .. } => *has_alternatives,
        }
    }

    /// Resolves the final [`Uri`] for an outgoing request, combining the target [`Uri`] with
    /// the [`BaseUri`] produced by this routing according to the configured
    /// [`BaseUriConflict`] policy.
    ///
    /// # Errors
    ///
    /// Returns [`HttpError::validation`] when:
    /// - the target [`Uri`] has no [`BaseUri`] and the routing does not produce one
    ///   (regardless of the configured [`BaseUriConflict`] policy); or
    /// - the policy is [`BaseUriConflict::Fail`] and the target [`Uri`] already has a
    ///   [`BaseUri`] while the routing also produces one.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "while not consuming the context, we might do it at some point"
    )]
    pub fn resolve_uri(&self, context: RouterContext, uri: Uri) -> Result<Uri, HttpError> {
        let (original, path) = uri.into_parts();
        let routed = self.resolve_base_uri(&context);

        // if new base uri is not available, return existing uri
        let Some(routed) = routed else {
            let Some(original) = original else {
                return Err(HttpError::validation_with_label(
                    "the target URI has no base URI and the routing did not produce one; \
                     provide a base URI on the target or configure the router to resolve one",
                    LABEL_URI_MISSING,
                ));
            };
            return Ok(Uri::from_parts(Some(original), path));
        };

        // if existing base uri is not available, return new base uri
        let Some(original) = original else {
            return Ok(Uri::from_parts(routed, path));
        };

        // choose base uri based on conflict policy
        let chosen = match self.conflict_policy {
            BaseUriConflict::UseOriginal => original,
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

    /// Resolves the [`HttpRequest`]'s URI in place by routing it through
    /// [`Router::resolve_uri`].
    ///
    /// Routes from [`RequestUris::original`] when the extension is present
    /// (attached by [`HttpRequestBuilder::build`][crate::HttpRequestBuilder::build]),
    /// or from the request's current [`http::Uri`] otherwise. On success,
    /// updates the request's [`http::Uri`] and the
    /// [`routed`](RequestUris::routed) field, attaching a [`RequestUris`] if
    /// none was present. [`original`](RequestUris::original) is never
    /// overwritten, so repeated calls always re-route from the caller's
    /// untouched target.
    ///
    /// # Errors
    ///
    /// Returns [`HttpError::validation`] when:
    ///
    /// - the request's existing URI cannot be converted to a [`Uri`],
    /// - [`Router::resolve_uri`] fails (e.g., a [`BaseUriConflict::Fail`] conflict), or
    /// - the resolved [`Uri`] cannot be converted back to an [`http::Uri`].
    pub fn resolve_request_uri(&self, context: RouterContext, request: &mut HttpRequest) -> Result<(), HttpError> {
        // Always route from the caller-supplied target so repeated calls
        // are idempotent. Clone rather than take, so a failure below leaves
        // the request unchanged.
        let original: Uri = match request.extensions().get::<RequestUris>() {
            Some(uris) => uris.original().clone(),
            None => request.uri().clone().try_into()?,
        };
        let resolved = self.resolve_uri(context.with_request(request), original.clone())?;
        let http_uri = resolved.clone().try_into()?;

        // Commit: update the request's URI and record the resolved URI.
        // Only `routed` is mutated; `original` is preserved across attempts.
        *request.uri_mut() = http_uri;
        if let Some(uris) = request.extensions_mut().get_mut::<RequestUris>() {
            uris.set_routed(resolved);
        } else {
            let mut uris = RequestUris::new(original);
            uris.set_routed(resolved);
            request.extensions_mut().insert(uris);
        }

        Ok(())
    }

    /// Resolves the [`BaseUri`] for the current request, if any.
    fn resolve_base_uri(&self, context: &RouterContext) -> Option<BaseUri> {
        match self.resolver.as_ref() {
            Resolver::Empty => None,
            Resolver::Fixed(base_uri) => Some(base_uri.clone()),
            Resolver::Custom { resolver, .. } => resolver(context),
        }
    }
}

type RouterFn = dyn Fn(&RouterContext) -> Option<BaseUri> + Send + Sync + 'static;

#[derive(Default)]
enum Resolver {
    #[default]
    Empty,
    Fixed(BaseUri),
    Custom {
        resolver: Arc<RouterFn>,
        has_alternatives: bool,
    },
}

impl std::fmt::Debug for Resolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("Empty"),
            Self::Fixed(base_uri) => f.debug_tuple("Fixed").field(base_uri).finish(),
            Self::Custom { has_alternatives, .. } => f
                .debug_struct("Custom")
                .field("has_alternatives", has_alternatives)
                .finish_non_exhaustive(),
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
    fn default_passes_target_through_when_target_has_base() {
        let router = Router::default();

        let with_base = router.resolve_uri(RouterContext::new(), target_with_base()).unwrap();
        assert_eq!(with_base.to_string().declassify_into(), "https://existing.example.com/items");
    }

    #[test]
    fn default_errors_when_target_has_no_base() {
        let router = Router::default();

        let err = router.resolve_uri(RouterContext::new(), target_without_base()).unwrap_err();
        assert_eq!(err.label(), "uri_missing");
    }

    #[test]
    fn fixed_attaches_when_target_has_none() {
        let router = Router::fixed(BaseUri::from_static("https://api.example.com"));
        let resolved = router.resolve_uri(RouterContext::new(), target_without_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://api.example.com/v1/items");
    }

    #[test]
    fn custom_resolver_returning_none_passes_through() {
        let router = Router::custom(|_| None, false);
        let resolved = router.resolve_uri(RouterContext::new(), target_with_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://existing.example.com/items");
    }

    #[test]
    fn custom_resolver_returning_some_is_used() {
        let router = Router::custom(|_| Some(BaseUri::from_static("https://api.example.com")), false);
        let resolved = router.resolve_uri(RouterContext::new(), target_without_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://api.example.com/v1/items");
    }

    #[test]
    fn keep_existing_is_default_on_conflict() {
        let router = Router::fixed(BaseUri::from_static("https://api.example.com"));

        let resolved = router.resolve_uri(RouterContext::new(), target_with_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://existing.example.com/items");
    }

    #[test]
    fn use_routed_replaces_original_base_uri() {
        let router = Router::fixed(BaseUri::from_static("https://api.example.com")).conflict_policy(BaseUriConflict::UseRouted);

        let resolved = router.resolve_uri(RouterContext::new(), target_with_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://api.example.com/items");
    }

    #[test]
    fn fail_returns_error_on_conflict() {
        let router = Router::fixed(BaseUri::from_static("https://api.example.com")).conflict_policy(BaseUriConflict::Fail);

        let err = router.resolve_uri(RouterContext::new(), target_with_base()).unwrap_err();
        assert_eq!(err.label(), "uri_conflict");
    }

    #[test]
    fn missing_base_uri_errors_regardless_of_policy() {
        // No base URI on the target and no routing means there is no base URI to use,
        // which is always invalid regardless of the conflict policy.
        for policy in [BaseUriConflict::UseOriginal, BaseUriConflict::UseRouted, BaseUriConflict::Fail] {
            let router = Router::default().conflict_policy(policy);

            let err = router.resolve_uri(RouterContext::new(), Uri::default()).unwrap_err();
            assert_eq!(err.label(), "uri_missing", "empty Uri with policy {policy:?}");

            let err = router.resolve_uri(RouterContext::new(), target_without_base()).unwrap_err();
            assert_eq!(err.label(), "uri_missing", "path-only Uri with policy {policy:?}");
        }
    }

    #[test]
    fn fail_does_not_trigger_without_conflict() {
        let router = Router::fixed(BaseUri::from_static("https://api.example.com")).conflict_policy(BaseUriConflict::Fail);

        let resolved = router.resolve_uri(RouterContext::new(), target_without_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://api.example.com/v1/items");
    }

    #[test]
    fn fallback_uses_primary_without_previous_recovery() {
        let router = Router::fallback(
            BaseUri::from_static("https://primary.example.com"),
            BaseUri::from_static("https://fallback.example.com"),
        );
        let resolved = router.resolve_uri(RouterContext::new(), target_without_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://primary.example.com/v1/items");
    }

    #[test]
    fn fallback_uses_primary_when_previous_recovery_is_not_unavailable() {
        let router = Router::fallback(
            BaseUri::from_static("https://primary.example.com"),
            BaseUri::from_static("https://fallback.example.com"),
        );
        let ctx = RouterContext::new().with_previous_recovery(recoverable::RecoveryInfo::retry());
        let resolved = router.resolve_uri(ctx, target_without_base()).unwrap();
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
        let resolved = router.resolve_uri(ctx, target_without_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://fallback.example.com/v1/items");
    }

    #[test]
    fn fallback_uses_fallback_on_last_attempt_after_first() {
        let router = Router::fallback(
            BaseUri::from_static("https://primary.example.com"),
            BaseUri::from_static("https://fallback.example.com"),
        );
        let ctx = RouterContext::new().with_attempt(2, true);
        let resolved = router.resolve_uri(ctx, target_without_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://fallback.example.com/v1/items");
    }

    #[test]
    fn fallback_uses_primary_on_first_attempt_even_when_last() {
        let router = Router::fallback(
            BaseUri::from_static("https://primary.example.com"),
            BaseUri::from_static("https://fallback.example.com"),
        );
        let ctx = RouterContext::new().with_attempt(0, true);
        let resolved = router.resolve_uri(ctx, target_without_base()).unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://primary.example.com/v1/items");
    }

    #[test]
    fn fallback_switches_endpoint_across_in_place_resolve_request_uri_calls() {
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
        router.resolve_request_uri(RouterContext::new(), &mut request).unwrap();
        assert_eq!(request.uri().to_string(), "https://primary.example.com/v1/items");

        // Second attempt: previous recovery reports unavailable, fallback wins
        // and must replace the previously-attached primary base URI on the request.
        let ctx = RouterContext::new()
            .with_previous_recovery(recoverable::RecoveryInfo::unavailable())
            .with_attempt(1, false);
        router.resolve_request_uri(ctx, &mut request).unwrap();
        assert_eq!(request.uri().to_string(), "https://fallback.example.com/v1/items");
    }

    #[test]
    fn fallback_does_not_duplicate_base_path_across_in_place_resolve_request_uri_calls() {
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
        router.resolve_request_uri(RouterContext::new(), &mut request).unwrap();
        assert_eq!(request.uri().to_string(), "https://primary.example.com/api/v1/items");

        // Second attempt: fallback endpoint, not stacked on the previously-routed path.
        let ctx = RouterContext::new()
            .with_previous_recovery(recoverable::RecoveryInfo::unavailable())
            .with_attempt(1, false);
        router.resolve_request_uri(ctx, &mut request).unwrap();
        assert_eq!(request.uri().to_string(), "https://fallback.example.com/api/items");

        // Third attempt: back to primary, still clean.
        let ctx = RouterContext::new().with_attempt(2, false);
        router.resolve_request_uri(ctx, &mut request).unwrap();
        assert_eq!(request.uri().to_string(), "https://primary.example.com/api/v1/items");
    }

    #[test]
    fn fallback_uses_primary_on_non_last_attempt() {
        let router = Router::fallback(
            BaseUri::from_static("https://primary.example.com"),
            BaseUri::from_static("https://fallback.example.com"),
        );
        let ctx = RouterContext::new().with_attempt(1, false);
        let resolved = router.resolve_uri(ctx, target_without_base()).unwrap();
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
        let router = Router::custom(|_| None, true);
        assert!(router.has_alternatives());
    }

    #[test]
    fn custom_without_alternatives_reports_false() {
        let router = Router::custom(|_| None, false);
        assert!(!router.has_alternatives());
    }

    #[test]
    fn resolve_request_uri_falls_back_to_request_uri_without_request_uris_extension() {
        // Hand-built requests (constructed directly via `http::Request::new`)
        // do not carry the `RequestUris` extension that
        // `HttpRequestBuilder::build` attaches. In that case
        // `resolve_request_uri` must fall back to converting the request's
        // current `http::Uri` and route from that, and then attach a
        // `RequestUris` so subsequent calls remain idempotent.
        let router = Router::fixed(BaseUri::from_static("https://api.example.com"));
        let body = crate::HttpBodyBuilder::new_fake().empty();
        let mut request = http::Request::new(body);
        *request.uri_mut() = http::Uri::from_static("/v1/items");
        assert!(
            request.extensions().get::<RequestUris>().is_none(),
            "precondition: no RequestUris extension"
        );

        router.resolve_request_uri(RouterContext::new(), &mut request).unwrap();

        assert_eq!(request.uri().to_string(), "https://api.example.com/v1/items");
        let uris = request
            .extensions()
            .get::<RequestUris>()
            .expect("resolve_request_uri must attach a RequestUris extension for hand-built requests");
        assert_eq!(uris.original().to_string().declassify_ref(), "/v1/items");
        assert_eq!(
            uris.routed().expect("routed must be populated").to_string().declassify_ref(),
            "https://api.example.com/v1/items"
        );
    }

    #[test]
    fn resolve_request_uri_attaches_base_uri() {
        let router = Router::fixed(BaseUri::from_static("https://api.example.com"));
        let mut request = crate::HttpRequestBuilder::new_fake().get("/v1/items").build().unwrap();

        router.resolve_request_uri(RouterContext::new(), &mut request).unwrap();

        assert_eq!(request.uri().to_string(), "https://api.example.com/v1/items");
    }

    #[test]
    fn resolve_request_uri_attaches_resolved_uri_extension() {
        // After `resolve_request_uri`, the `RequestUris` extension's `routed`
        // field carries the resolved `Uri` (with the routed `BaseUri`
        // applied), while `original` preserves the caller's untouched target
        // so subsequent re-routings (e.g., recovery attempts) start from it.
        let router = Router::fixed(BaseUri::from_static("https://api.example.com"));
        let mut request = crate::HttpRequestBuilder::new_fake().get("/v1/items").build().unwrap();

        router.resolve_request_uri(RouterContext::new(), &mut request).unwrap();

        let uris = request
            .extensions()
            .get::<RequestUris>()
            .expect("resolve_request_uri must keep the RequestUris extension");
        assert_eq!(uris.original().to_string().declassify_ref(), "/v1/items");
        assert_eq!(
            uris.routed().expect("routed must be populated").to_string().declassify_ref(),
            "https://api.example.com/v1/items"
        );
    }

    #[test]
    fn resolve_request_uri_preserves_original_across_repeated_calls() {
        // Regression test for the previously documented contract bug: when
        // `resolve_request_uri` overwrote the templated `Uri` extension with
        // the resolved URI, repeated calls would route from the previously
        // routed result instead of from the caller's original target. With
        // the `UseOriginal` policy and a target that has no base URI, this
        // caused the second call to silently keep the first routed base
        // even when the router would have produced a different one.
        let cell = std::sync::Arc::new(std::sync::Mutex::new(0_usize));
        let cell_clone = cell;
        let router = Router::custom(
            move |_| {
                let mut count = cell_clone.lock().unwrap();
                let base = if *count == 0 {
                    BaseUri::from_static("https://first.example.com")
                } else {
                    BaseUri::from_static("https://second.example.com")
                };
                *count += 1;
                Some(base)
            },
            true,
        );
        let mut request = crate::HttpRequestBuilder::new_fake().get("/v1/items").build().unwrap();

        router.resolve_request_uri(RouterContext::new(), &mut request).unwrap();
        assert_eq!(request.uri().to_string(), "https://first.example.com/v1/items");

        // Second call must re-route from the caller's original `/v1/items`,
        // not from the previously routed `https://first.example.com/v1/items`.
        router.resolve_request_uri(RouterContext::new(), &mut request).unwrap();
        assert_eq!(request.uri().to_string(), "https://second.example.com/v1/items");

        // And `original` is preserved across calls.
        let uris = request.extensions().get::<RequestUris>().unwrap();
        assert_eq!(uris.original().to_string().declassify_ref(), "/v1/items");
    }

    #[test]
    fn resolve_request_uri_keeps_existing_base_uri_by_default() {
        let router = Router::fixed(BaseUri::from_static("https://api.example.com"));
        let mut request = crate::HttpRequestBuilder::new_fake()
            .get("https://existing.example.com/items")
            .build()
            .unwrap();

        router.resolve_request_uri(RouterContext::new(), &mut request).unwrap();

        assert_eq!(request.uri().to_string(), "https://existing.example.com/items");
    }

    #[test]
    fn resolve_request_uri_returns_error_on_conflict_when_policy_is_fail() {
        let router = Router::fixed(BaseUri::from_static("https://api.example.com")).conflict_policy(BaseUriConflict::Fail);
        let mut request = crate::HttpRequestBuilder::new_fake()
            .get("https://existing.example.com/items")
            .build()
            .unwrap();

        let err = router.resolve_request_uri(RouterContext::new(), &mut request).unwrap_err();
        assert_eq!(err.label(), "uri_conflict");
    }

    #[test]
    fn resolve_request_uri_preserves_original_uri_on_failure() {
        let router = Router::fixed(BaseUri::from_static("https://api.example.com")).conflict_policy(BaseUriConflict::Fail);
        let mut request = crate::HttpRequestBuilder::new_fake()
            .get("https://existing.example.com/items")
            .build()
            .unwrap();

        let original_uri = request.uri().clone();
        let _ = router.resolve_request_uri(RouterContext::new(), &mut request).unwrap_err();

        assert_eq!(request.uri(), &original_uri);
    }

    #[test]
    fn resolver_debug_format() {
        assert_eq!(format!("{:?}", Resolver::Empty), "Empty");

        let fixed = Resolver::Fixed(BaseUri::from_static("https://api.example.com"));
        assert!(format!("{fixed:?}").starts_with("Fixed("));

        let custom = Resolver::Custom {
            resolver: Arc::new(|_| None),
            has_alternatives: true,
        };
        let custom_debug = format!("{custom:?}");
        assert!(custom_debug.starts_with("Custom"));
        assert!(custom_debug.contains("has_alternatives: true"));
    }
}
