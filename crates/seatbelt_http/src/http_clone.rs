// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use http::Method;
use http_extensions::routing::{Router, RouterContext};
use http_extensions::{HttpRequest, HttpRequestExt, RequestExt};
use seatbelt::{Attempt, RecoveryInfo};

/// A strategy for cloning HTTP requests for retries or hedging.
///
/// Limits cloning (and therefore retry/hedging attempts) to specific method
/// categories. The default is [`HttpClone::safe_only`].
///
/// If the [`HttpRequest`] extensions contain a [`Router`], it is used to
/// re-resolve the request URI on every attempt after the first. If routing
/// fails for a subsequent attempt, the clone is dropped so the caller can
/// skip that attempt.
#[derive(Debug, Default)]
pub struct HttpClone(Inner);

impl HttpClone {
    /// Clone requests for all HTTP methods.
    #[must_use]
    pub fn all() -> Self {
        Self(Inner::All)
    }

    /// Clone only idempotent methods (e.g. `GET`, `HEAD`, `PUT`, `DELETE`,
    /// `OPTIONS`).
    #[must_use]
    pub fn idempotent() -> Self {
        Self(Inner::Idempotent)
    }

    /// Clone only safe methods (e.g. `GET`, `HEAD`, `OPTIONS`). This is the
    /// default.
    #[cfg_attr(test, mutants::skip)] // SafeOnly is the default, so skip in mutation testing
    #[must_use]
    pub fn safe_only() -> Self {
        Self(Inner::SafeOnly)
    }

    pub(super) fn try_clone(
        &self,
        request: &mut HttpRequest,
        attempt: Attempt,
        previous_recovery: Option<&RecoveryInfo>,
    ) -> Option<HttpRequest> {
        let mut result = if self.can_clone(request.method()) {
            request.try_clone()
        } else {
            None
        };

        // Apply per-attempt updates to the clone when available, otherwise to the original
        // (e.g., non-cloneable streamed requests) so attempt tracking and routing stay accurate
        // across retries.
        attach_attempt(result.as_mut().unwrap_or(request), attempt);
        if !update_request_uri(result.as_mut().unwrap_or(request), attempt, previous_recovery) {
            // Routing failed; drop the clone so the caller doesn't retry against an unresolved URI.
            return None;
        }

        result
    }

    fn can_clone(&self, method: &Method) -> bool {
        match self.0 {
            Inner::All => true,
            Inner::Idempotent => method.is_idempotent(),
            Inner::SafeOnly => method.is_safe(),
        }
    }
}

/// Re-routes `request` for the given retry `attempt`.
#[must_use]
fn update_request_uri(request: &mut HttpRequest, attempt: Attempt, previous_recovery: Option<&RecoveryInfo>) -> bool {
    //  The routing only applies if:
    //
    // - Router is available
    // - The router has alternatives, meaning there are multiple URIs to choose from
    // - The attempt is not the first one (we assume caller already applied router for first attempt)
    let router = match request.extensions().get::<Router>() {
        Some(router) if router.has_alternatives() && !attempt.is_first() => router.clone(),
        _ => return true,
    };

    let mut context = RouterContext::new().with_attempt(attempt);
    if let Some(previous_recovery) = previous_recovery {
        context = context.with_previous_recovery(previous_recovery.clone());
    }

    // Update the request URI based on the router's resolution.
    router.resolve_request_uri(context, request).is_ok()
}

#[derive(Debug, Default)]
enum Inner {
    #[default]
    SafeOnly,
    Idempotent,
    All,
}

fn attach_attempt(request: &mut HttpRequest, attempt: Attempt) {
    request.set_attempt(attempt);
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use http_extensions::HttpRequestBuilder;
    use http_extensions::routing::BaseUriConflict;
    use templated_uri::BaseUri;

    use super::*;

    #[test]
    fn test_default_is_safe_only() {
        let clone = HttpClone::default();

        // Safe methods should be cloneable
        assert!(clone.can_clone(&Method::GET));
        assert!(clone.can_clone(&Method::HEAD));
        assert!(clone.can_clone(&Method::OPTIONS));

        // Unsafe methods should not be cloneable
        assert!(!clone.can_clone(&Method::POST));
        assert!(!clone.can_clone(&Method::PUT));
        assert!(!clone.can_clone(&Method::DELETE));
        assert!(!clone.can_clone(&Method::PATCH));
    }

    #[test]
    fn test_safe_only_strategy() {
        let clone = HttpClone::safe_only();

        // Safe methods
        assert!(clone.can_clone(&Method::GET));
        assert!(clone.can_clone(&Method::HEAD));
        assert!(clone.can_clone(&Method::OPTIONS));

        // Unsafe methods (even if idempotent)
        assert!(!clone.can_clone(&Method::PUT));
        assert!(!clone.can_clone(&Method::DELETE));
        assert!(!clone.can_clone(&Method::POST));
        assert!(!clone.can_clone(&Method::PATCH));
    }

    #[test]
    fn test_idempotent_strategy() {
        let clone = HttpClone::idempotent();

        // Idempotent methods should be cloneable
        assert!(clone.can_clone(&Method::GET));
        assert!(clone.can_clone(&Method::HEAD));
        assert!(clone.can_clone(&Method::PUT));
        assert!(clone.can_clone(&Method::DELETE));
        assert!(clone.can_clone(&Method::OPTIONS));

        // Non-idempotent methods should not be cloneable
        assert!(!clone.can_clone(&Method::POST));
        assert!(!clone.can_clone(&Method::PATCH));
    }

    #[test]
    fn test_all_strategy() {
        let clone = HttpClone::all();

        // All methods should be cloneable
        assert!(clone.can_clone(&Method::GET));
        assert!(clone.can_clone(&Method::HEAD));
        assert!(clone.can_clone(&Method::POST));
        assert!(clone.can_clone(&Method::PUT));
        assert!(clone.can_clone(&Method::DELETE));
        assert!(clone.can_clone(&Method::PATCH));
        assert!(clone.can_clone(&Method::OPTIONS));
        assert!(clone.can_clone(&Method::CONNECT));
        assert!(clone.can_clone(&Method::TRACE));
    }

    #[test]
    fn test_custom_method() {
        let clone_safe = HttpClone::safe_only();
        let clone_idempotent = HttpClone::idempotent();
        let clone_all = HttpClone::all();

        // Test with a custom method (not one of the standard ones)
        let custom_method = Method::from_bytes(b"CUSTOM").unwrap();

        // Custom methods are typically not safe
        assert!(!clone_safe.can_clone(&custom_method));

        // Custom methods are typically not idempotent
        assert!(!clone_idempotent.can_clone(&custom_method));

        // But "all" strategy should allow them
        assert!(clone_all.can_clone(&custom_method));
    }

    #[test]
    fn try_clone_attaches_attempt_to_cloned_request() {
        let clone = HttpClone::all();
        let attempt = Attempt::new(3, false);

        let mut request = HttpRequestBuilder::new_fake()
            .method(Method::GET)
            .uri("https://example.com")
            .build()
            .unwrap();

        let cloned = clone.try_clone(&mut request, attempt, None);

        let cloned = cloned.expect("cloneable request should produce Some");
        let attached = cloned.attempt().expect("attempt should be attached to the cloned request");
        assert_eq!(attached.index(), 3);
        assert!(!attached.is_last());
    }

    #[test]
    fn try_clone_returns_none_when_routing_fails() {
        // Router has alternatives and a `Fail` conflict policy. Combined with a
        // request whose target URI already carries a base URI, resolving on any
        // non-first attempt must fail, causing `try_clone` to drop the clone.
        let router =
            Router::custom(|_| Some(BaseUri::from_static("https://routed.example.com")), true).conflict_policy(BaseUriConflict::Fail);

        let mut request = HttpRequestBuilder::new_fake()
            .method(Method::GET)
            .uri("https://existing.example.com/items")
            .extension(router)
            .build()
            .unwrap();

        let clone = HttpClone::all();
        // Non-first attempt triggers re-routing inside `try_clone`.
        let attempt = Attempt::new(1, false);

        let result = clone.try_clone(&mut request, attempt, None);

        assert!(result.is_none(), "failed routing should drop the clone");
    }

    #[test]
    fn try_clone_attaches_attempt_to_original_when_clone_is_disallowed() {
        let clone = HttpClone::safe_only();
        let attempt = Attempt::new(1, true);

        // POST is not safe, so safe_only will refuse to clone it.
        let mut request = HttpRequestBuilder::new_fake()
            .method(Method::POST)
            .uri("https://example.com")
            .build()
            .unwrap();

        let result = clone.try_clone(&mut request, attempt, None);

        assert!(result.is_none(), "unsafe method should not be cloned");
        let attached = request.attempt().expect("attempt should be attached to the original request");
        assert_eq!(attached.index(), 1);
        assert!(attached.is_last());
    }
}
