// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The trait describing the shape of a `rest_over_grpc::build`-generated
//! `Transcoder`.

use crate::transcoding::{HttpResponse, TranscodeResponse};

/// The transcode surface of a generated `Transcoder`: resolve an HTTP request to
/// a service handler, transcode JSON⇄protobuf, and return a
/// [`TranscodeResponse`].
///
/// Every `Transcoder` emitted by [`rest_over_grpc::build`](crate::build)
/// implements this trait, so you can write code — middleware, adapters, test
/// harnesses — generic over any generated transcoder. `target` is the request
/// path plus optional `?query`; `headers` are the request-side gRPC metadata
/// (the `Accept` header drives server-streaming content negotiation); `body` is
/// the raw request body.
///
/// A single [`TranscodeResponse`] covers both RPC shapes: a **unary** RPC yields
/// a buffered [`TranscodeResponse::Unary`](crate::transcoding::TranscodeResponse::Unary),
/// a **server-streaming** RPC yields a
/// [`TranscodeResponse::Streaming`](crate::transcoding::TranscodeResponse::Streaming)
/// whose frames reach the wire incrementally.
///
/// A shared transcoder works too: [`Arc<D>`](std::sync::Arc) forwards to `D`, so
/// an `Arc`-wrapped transcoder can be cloned cheaply into an adapter.
pub trait Transcode: Sync {
    /// Resolves `method` + `target` to a generated route, transcodes the request
    /// into the RPC's message, invokes the handler, and transcodes the reply back
    /// to JSON.
    ///
    /// # Parameters
    ///
    /// - `method` — the request's HTTP method (e.g. `"GET"`, `"POST"`), matched
    ///   against each route's configured method.
    /// - `target` — the request target: the path plus an optional `?query`
    ///   (origin form, e.g. `/v1/shelves/7?filter=fiction`). The path selects the
    ///   route and binds its `{path}` variables; query parameters bind to the
    ///   remaining request-message fields.
    /// - `headers` — the request headers, which double as the request-side gRPC
    ///   metadata; they are moved into the [`Context`](crate::handling::Context) the
    ///   handler sees. The `Accept` header among them drives server-streaming
    ///   response content negotiation.
    /// - `body` — the raw request body bytes (JSON), decoded into the request
    ///   message according to the route's body mapping.
    ///
    /// # Returns
    ///
    /// `Some(response)` when a route matched — including when transcoding or the
    /// handler fails, in which case the [`TranscodeResponse`] carries the mapped
    /// error status (as a [`Unary`](crate::transcoding::TranscodeResponse::Unary)
    /// response). `None` when no route matched, so a caller can fall through to
    /// its own handling (a custom `404`, hand-written routes, …).
    fn try_transcode(
        &self,
        method: &str,
        target: &str,
        headers: http::HeaderMap,
        body: &[u8],
    ) -> impl core::future::Future<Output = Option<TranscodeResponse>> + Send;

    /// Like [`try_transcode`](Self::try_transcode), but answers an unmatched route
    /// with a `404` (a [`TranscodeResponse::Unary`](crate::transcoding::TranscodeResponse::Unary))
    /// instead of `None`.
    ///
    /// The parameters are identical to [`try_transcode`](Self::try_transcode):
    /// `method` + `target` select the route, `headers` become the request
    /// metadata, and `body` is the raw request body. The convenience wrapper for
    /// when you have no fallback to compose.
    fn transcode(
        &self,
        method: &str,
        target: &str,
        headers: http::HeaderMap,
        body: &[u8],
    ) -> impl core::future::Future<Output = TranscodeResponse> + Send {
        async move {
            self.try_transcode(method, target, headers, body)
                .await
                .unwrap_or_else(|| TranscodeResponse::Unary(HttpResponse::not_found()))
        }
    }
}

/// Forwards to the wrapped transcoder, so an `Arc`-shared `Transcoder` (cloned
/// cheaply into an adapter) still implements [`Transcode`].
impl<D: Transcode + Send> Transcode for std::sync::Arc<D> {
    fn try_transcode(
        &self,
        method: &str,
        target: &str,
        headers: http::HeaderMap,
        body: &[u8],
    ) -> impl core::future::Future<Output = Option<TranscodeResponse>> + Send {
        (**self).try_transcode(method, target, headers, body)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    /// A minimal transcoder that matches no route, used to check that the `Arc`
    /// forwarding impl delegates to the wrapped transcoder.
    struct NoRoute;

    impl Transcode for NoRoute {
        fn try_transcode(
            &self,
            _method: &str,
            _target: &str,
            _headers: http::HeaderMap,
            _body: &[u8],
        ) -> impl core::future::Future<Output = Option<TranscodeResponse>> + Send {
            core::future::ready(None)
        }
    }

    #[test]
    fn arc_forwards_try_transcode() {
        let transcoder = Arc::new(NoRoute);
        let result = futures::executor::block_on(transcoder.try_transcode("GET", "/x", http::HeaderMap::new(), b""));
        assert!(result.is_none());
    }

    /// A transcoder that always answers with a server-streaming response, used to
    /// exercise `transcode`'s streaming branch.
    struct StreamingRoute;

    impl Transcode for StreamingRoute {
        fn try_transcode(
            &self,
            _method: &str,
            _target: &str,
            _headers: http::HeaderMap,
            _body: &[u8],
        ) -> impl core::future::Future<Output = Option<TranscodeResponse>> + Send {
            let items = futures_util::stream::iter(vec![Ok::<u32, crate::handling::Status>(1)]);
            let streaming = crate::transcoding::StreamingResponse::encode(items, crate::stream::StreamEncoding::JsonArray);
            core::future::ready(Some(TranscodeResponse::Streaming(streaming)))
        }
    }

    /// Classifies a [`TranscodeResponse`] as `("unary", status)` or
    /// `("streaming", 200 OK)`. Both `transcode` tests share it, so each variant
    /// arm is exercised by one of them — there is no arm a passing test can never
    /// take.
    fn classify(response: TranscodeResponse) -> (&'static str, http::StatusCode) {
        match response {
            TranscodeResponse::Unary(unary) => ("unary", unary.status()),
            TranscodeResponse::Streaming(_) => ("streaming", http::StatusCode::OK),
        }
    }

    #[test]
    fn transcode_answers_unmatched_route_with_a_404() {
        let response = futures::executor::block_on(NoRoute.transcode("GET", "/x", http::HeaderMap::new(), b""));
        assert_eq!(classify(response), ("unary", http::StatusCode::NOT_FOUND));
    }

    #[test]
    fn transcode_forwards_a_matched_streaming_response() {
        // A `Some(Streaming)` from `try_transcode` flows through the default
        // `transcode` unchanged, so the reply stays a streaming response.
        let response = futures::executor::block_on(StreamingRoute.transcode("GET", "/x", http::HeaderMap::new(), b""));
        assert_eq!(classify(response), ("streaming", http::StatusCode::OK));
    }
}
