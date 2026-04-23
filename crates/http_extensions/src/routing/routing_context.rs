// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use recoverable::RecoveryInfo;

use crate::HttpRequest;

/// Context passed to the closure of [`Routing::custom`] when resolving a [`BaseUri`].
///
/// This type is intentionally opaque so that fields can be added in the future without
/// breaking the closure signature. Use the accessors to inspect the available
/// information about the current request attempt.
///
/// [`Routing::custom`]: super::Routing::custom
/// [`BaseUri`]: templated_uri::BaseUri
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct RoutingContext<'a> {
    attempt: u32,
    is_last_attempt: bool,
    previous_recovery: Option<RecoveryInfo>,
    request: Option<&'a HttpRequest>,
}

impl<'a> RoutingContext<'a> {
    /// Creates a new [`RoutingContext`] for the first (and only) attempt.
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
    #[must_use]
    pub fn with_attempt(mut self, attempt: u32, is_last_attempt: bool) -> Self {
        self.attempt = attempt;
        self.is_last_attempt = is_last_attempt;
        self
    }

    /// Sets the [`RecoveryInfo`] produced by the previous attempt.
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
