// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use recoverable::Attempt;
use templated_uri::Uri;

use crate::UriTemplateLabel;

/// Request extension consolidating per-request metadata into a single value.
///
/// `RequestInfo` aggregates the metadata that resilience, routing, and observability
/// machinery attach to and read from a request. It currently carries the request's
/// templated URIs, an explicit URI template label, and the current resilience
/// [`Attempt`]. It is attached by
/// [`HttpRequestBuilder::build`][crate::HttpRequestBuilder::build] and accessed through
/// the [`RequestExt`][crate::RequestExt] methods (for example
/// [`request_info`][crate::RequestExt::request_info],
/// [`attempt`][crate::RequestExt::attempt], and
/// [`set_attempt`][crate::RequestExt::set_attempt]). Folding everything into one
/// extension avoids attaching (and looking up) several separate extensions.
///
/// All fields are optional and the type is [`Default`], so callers within the crate can
/// construct it with struct-update syntax:
///
/// ```ignore
/// let info = RequestInfo {
///     original_uri: Some(uri),
///     ..Default::default()
/// };
/// ```
///
/// The struct is `#[non_exhaustive]` so new metadata can be added without breaking callers.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct RequestInfo {
    /// The caller-supplied templated [`Uri`], preserved untouched across routing attempts.
    ///
    /// [`Router::resolve_request_uri`](crate::routing::Router::resolve_request_uri) always
    /// re-routes from this value so repeated routing calls are idempotent.
    pub original_uri: Option<Uri>,

    /// The most recently resolved [`Uri`], updated on each routing attempt.
    pub routed_uri: Option<Uri>,

    /// An explicit [`UriTemplateLabel`] for the request, taking precedence over any label
    /// or template derived from [`original_uri`](Self::original_uri).
    pub uri_template_label: Option<UriTemplateLabel>,

    /// The [`Attempt`] describing the current execution of the request within a
    /// resilience operation (e.g. retries or hedging), if known.
    pub attempt: Option<Attempt>,
}

impl RequestInfo {
    /// Returns the best-known templated [`Uri`] for the request: the
    /// [`routed_uri`](Self::routed_uri) if present, otherwise the
    /// [`original_uri`](Self::original_uri).
    #[must_use]
    pub fn resolved_uri(&self) -> Option<&Uri> {
        self.routed_uri.as_ref().or(self.original_uri.as_ref())
    }
}
