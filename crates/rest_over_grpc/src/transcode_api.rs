// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The trait describing the shape of a `rest_over_grpc::build`-generated
//! `Transcoder`.

use crate::transcoding::{HttpResponse, TranscodeResponse};
#[cfg(test)]
use crate::{stream::StreamEncoding, transcode_response::StreamingResponse};

/// Resolves HTTP requests, invokes generated service handlers, and returns
/// buffered or streaming JSON responses.
///
/// Generated transcoders and `Arc`-wrapped transcoders implement this trait.
pub trait Transcode: Sync {
    /// Resolves `method` + `target` to a generated route, transcodes the request
    /// into the RPC's message, invokes the handler, and transcodes the reply back
    /// to JSON.
    ///
    /// Returns `None` when no generated route matches.
    ///
    /// Use this instead of [`transcode`](Self::transcode) when composing
    /// generated routes with hand-written routes or a custom fallback. See the
    /// runnable [`custom_fallback` example].
    ///
    /// [`custom_fallback` example]: https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc_examples/examples/transcoding/custom_fallback.rs
    fn try_transcode(
        &self,
        method: &str,
        target: &str,
        headers: http::HeaderMap,
        body: &[u8],
    ) -> impl core::future::Future<Output = Option<TranscodeResponse>> + Send;

    /// Transcodes a request, returning a JSON `404` for an unmatched route.
    ///
    /// The runnable [`basic_transcode` example] demonstrates both unary and
    /// streaming responses.
    ///
    /// [`basic_transcode` example]: https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc_examples/examples/transcoding/basic_transcode.rs
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
            let streaming = StreamingResponse::encode(items, StreamEncoding::JsonArray);
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
