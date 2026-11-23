// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The request path of the server middleware.

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::Arc;

use futures::FutureExt;
use futures::future::Either;
use http::header::HOST;
use http_body::Body;
use http_body_util::BodyExt;
use http_extensions::body::{Content, StreamingBody};
use http_extensions::{HttpRequest, HttpResponse, RequestHandler, Result};
use observed::Event;
use parking_lot::Mutex;

use super::body_buffer::BodyBuffer;
use super::events::{ActiveRequests, RequestAbandoned, RequestComplete, RequestError};
use super::layer::Server;
use super::request::{HeaderValues, RequestAndResponseHeaders, RequestEnd, RouteValues, ServerRequest, UriAuthority};
use super::response_body::emit_when_body_ends;
use crate::Telemetry;
use crate::drop_guard::TelemetryDropGuard;
use crate::shared::{OTHER, known_scheme};

impl<T: RequestHandler> Telemetry<T> {
    /// The [`execute`](layered::Service::execute) of a server middleware.
    #[expect(
        clippy::too_many_lines,
        reason = "a request goes through each step here in order: the pass-through checks, the drop guard, the copy of the request body and the call of the handler"
    )]
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
                headers: (server.request_header_fields.is_some() || server.response_header_fields.is_some()).then(|| {
                    Box::new(RequestAndResponseHeaders {
                        request: server
                            .request_header_fields
                            .clone()
                            .map(|fields| HeaderValues::new(fields, input.headers())),
                        response: None,
                    })
                }),
                request_body_buffer: None,
                response_body_buffer: None,
            },
            &self.sink,
            &self.clock,
        );
        guard.request.add_active_request(1);
        if server.request_body_media_types.matches(input.headers()) {
            let buffer = Arc::new(Mutex::new(BodyBuffer::new(server.body_size_limit)));
            if !input.body().is_end_stream() {
                input = input.map(|body| {
                    body.map(|content| {
                        Ok(match content {
                            Content::Bytes(bytes) => {
                                buffer.lock().push(&bytes);
                                Content::Bytes(bytes)
                            }
                            Content::Streaming(inner) => {
                                let buffer = Arc::clone(&buffer);
                                Content::Streaming(StreamingBody::new(inner.inspect_frame(move |frame| {
                                    if let Some(data) = frame.data_ref() {
                                        buffer.lock().push(data);
                                    }
                                })))
                            }
                        })
                    })
                    .expect("map fails only for a consumed body, which is at its end, so the check above rules it out")
                });
            }
            guard.request.request_body_buffer = Some(buffer);
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
                        headers.response = server
                            .response_header_fields
                            .clone()
                            .map(|fields| HeaderValues::new(fields, response.headers()));
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
