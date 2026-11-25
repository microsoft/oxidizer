// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The request path of the server middleware.

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::Arc;

use futures::FutureExt;
use futures::future::Either;
use http::header::HOST;
use http_extensions::{HttpRequest, HttpResponse, RequestHandler, Result};
use observed::Event;

use super::events::{ActiveRequests, RequestAbandoned, RequestComplete, RequestError};
use super::layer::Server;
use super::request::{RequestEnd, RouteValues, ServerRequest, UriAuthority};
use super::response_body::emit_when_body_ends;
use crate::Telemetry;
use crate::body_buffer::BodyBuffer;
use crate::drop_guard::TelemetryDropGuard;
use crate::header_fields::RequestAndResponseHeaders;
use crate::request_body::RequestBodyBuffer;
use crate::shared::{OTHER, known_scheme};

impl<T: RequestHandler> Telemetry<T> {
    /// The [`execute`](layered::Service::execute) of a server middleware.
    pub(crate) fn execute_server(&self, server: &Server, mut input: HttpRequest) -> impl Future<Output = Result<HttpResponse>> + Send {
        if server.excluded_paths.contains(input.uri().path()) {
            return Either::Left(self.inner.execute(input));
        }

        if !self.is_interested(&[
            RequestComplete::DESCRIPTION,
            RequestError::DESCRIPTION,
            RequestAbandoned::DESCRIPTION,
            ActiveRequests::DESCRIPTION,
        ]) {
            return Either::Left(self.inner.execute(input));
        }

        let (scheme, default_port) = match input.uri().scheme_str().map(known_scheme) {
            Some(Some((scheme, default_port))) => (Some(scheme), Some(default_port)),
            Some(None) => (Some(OTHER), None),
            None => (None, None),
        };

        let mut guard = TelemetryDropGuard::new(
            ServerRequest {
                sink: self.sink.clone(),
                method: input.method().clone(),
                version: input.version(),
                scheme,
                uri_authority: match input.uri().authority() {
                    Some(authority) => Some(UriAuthority::Uri(authority.clone(), default_port)),
                    None => input.headers().get(HOST).cloned().map(UriAuthority::Host),
                },
                route_values: server.route_fields.as_ref().and_then(|fields| {
                    let path_and_query = input.uri().path_and_query()?.clone();
                    Some(RouteValues {
                        route_fields: Arc::clone(fields),
                        path_and_query,
                    })
                }),
                headers: RequestAndResponseHeaders::new(
                    server.request_header_fields.as_ref(),
                    server.response_header_fields.as_ref(),
                    input.headers(),
                ),
                request_body_buffer: RequestBodyBuffer::NotLogged,
                response_body_buffer: None,
            },
            &self.sink,
            &self.clock,
        );
        guard.request.add_active_request(1);
        if server.request_body_media_types.matches(input.headers()) {
            guard.request.request_body_buffer = RequestBodyBuffer::new(&mut input, server.body_size_limit);
        }
        // The middleware resumes each panic that it catches and never polls the handler again, so the
        // handler does not need to be unwind safe. The handler can panic in the call too, because
        // `Execute` runs the handler closure in `execute`.
        let response = match catch_unwind(AssertUnwindSafe(|| self.inner.execute(input))) {
            Ok(response) => response,
            Err(payload) => {
                guard.emit(RequestEnd::Panic(None));
                resume_unwind(payload)
            }
        };
        Either::Right(async move {
            match AssertUnwindSafe(response).catch_unwind().await {
                Ok(Ok(response)) => {
                    if let Some(headers) = &mut guard.request.headers {
                        headers.set_response(server.response_header_fields.as_ref(), response.headers());
                    }
                    if server.response_body_media_types.matches(response.headers()) {
                        guard.request.response_body_buffer = Some(BodyBuffer::new(server.body_size_limit));
                    }
                    Ok(emit_when_body_ends(guard, response))
                }
                Ok(Err(error)) => {
                    guard.emit(RequestEnd::Error(&error, None));
                    Err(error)
                }
                Err(payload) => {
                    guard.emit(RequestEnd::Panic(None));
                    resume_unwind(payload)
                }
            }
        })
    }
}
