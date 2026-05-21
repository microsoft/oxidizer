// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use recoverable::RecoveryInfo;

use crate::HttpRequest;

/// Context passed to the closure of [`Router::custom`](super::Router::custom) when resolving a [`BaseUri`](templated_uri::BaseUri).
///
/// This type is intentionally opaque so that fields can be added in the future without
/// breaking the closure signature. Use the getters to inspect the available
/// information about the current request attempt.
///
/// A [`RouterContext`] is built and populated by the caller driving the
/// request (typically a resilience layer wrapping the HTTP client) and passed
/// to [`Router::resolve_request_uri`](super::Router::resolve_request_uri) /
/// [`Router::resolve_uri`](super::Router::resolve_uri) on every attempt. See
/// the [module-level "Recovery Context" section](super#recovery-context) for the
/// bigger picture: where this information comes from, who is responsible for
/// providing it, and how resolver like [`Router::fallback`](super::Router::fallback)
/// consume it.
#[derive(Debug, Clone)]
pub struct RouterContext<'a> {
    attempt: u32,
    is_last_attempt: bool,
    previous_recovery: Option<RecoveryInfo>,
    request: Option<&'a HttpRequest>,
}

impl Default for RouterContext<'_> {
    fn default() -> Self {
        Self {
            attempt: 0,
            is_last_attempt: true,
            previous_recovery: None,
            request: None,
        }
    }
}

impl<'a> RouterContext<'a> {
    /// Creates a new [`RouterContext`] for the first (and only) attempt.
    ///
    /// The returned context reports attempt index `0` and `is_last_attempt = true`.
    /// Use [`RouterContext::with_attempt`] to override these values when the
    /// request is part of a multi-attempt flow.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the [`HttpRequest`] associated with the current attempt.
    #[must_use]
    pub fn with_request(mut self, request: &'a HttpRequest) -> Self {
        self.request = Some(request);
        self
    }

    /// Sets the zero-based index of the current attempt and whether it is the
    /// last one that will be performed.
    ///
    /// The first attempt has index `0`, the second `1`, and so on.
    ///
    /// This is typically called by the resilience layer driving the recovery
    /// loop. See the [module-level "Recovery Context" section](super#recovery-context)
    /// for context.
    #[must_use]
    pub fn with_attempt(mut self, attempt: u32, is_last_attempt: bool) -> Self {
        self.attempt = attempt;
        self.is_last_attempt = is_last_attempt;
        self
    }

    /// Sets the [`RecoveryInfo`] produced by the previous attempt.
    ///
    /// This is typically called by the resilience layer driving the recovery
    /// loop, using the [`Recovery`](recoverable::Recovery) information
    /// attached to the error returned by the prior attempt. See the
    /// [module-level "Recovery Context" section](super#recovery-context) for context.
    #[must_use]
    pub fn with_previous_recovery(mut self, previous_recovery: RecoveryInfo) -> Self {
        self.previous_recovery = Some(previous_recovery);
        self
    }

    /// Returns the zero-based index of the current attempt.
    #[must_use]
    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    /// Returns `true` when the current attempt is the last one that will be performed.
    #[must_use]
    pub fn is_last_attempt(&self) -> bool {
        self.is_last_attempt
    }

    /// Returns the [`RecoveryInfo`] produced by the previous attempt, if any.
    #[must_use]
    pub fn previous_recovery(&self) -> Option<&RecoveryInfo> {
        self.previous_recovery.as_ref()
    }

    /// Returns the [`HttpRequest`] associated with the current attempt, if any.
    #[must_use]
    pub fn request(&self) -> Option<&'a HttpRequest> {
        self.request
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::HttpRequestBuilder;

    #[test]
    fn defaults() {
        let ctx = RouterContext::new();
        assert_eq!(ctx.attempt(), 0);
        assert!(ctx.is_last_attempt());
        assert!(ctx.previous_recovery().is_none());
        assert!(ctx.request().is_none());
        // Exercise Debug + Clone for coverage.
        let _ = format!("{:?}", ctx.clone());
    }

    #[test]
    fn with_setters() {
        let request = HttpRequestBuilder::new_fake()
            .get("https://example.com/")
            .build()
            .expect("valid request");
        let recovery = RecoveryInfo::retry();

        let ctx = RouterContext::new()
            .with_request(&request)
            .with_attempt(2, true)
            .with_previous_recovery(recovery);

        assert_eq!(ctx.attempt(), 2);
        assert!(ctx.is_last_attempt());
        assert!(ctx.previous_recovery().is_some());
        assert!(ctx.request().is_some());
    }

    #[test]
    fn with_attempt_can_set_is_last_attempt_to_false() {
        let ctx = RouterContext::new().with_attempt(1, false);
        assert_eq!(ctx.attempt(), 1);
        assert!(!ctx.is_last_attempt());
    }
}
