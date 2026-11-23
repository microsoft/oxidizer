// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The request path of the client middleware.

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::Arc;

use futures::FutureExt;
use futures::future::Either;
use http_extensions::{HttpRequest, HttpResponse, RequestExt, RequestHandler, Result, UriTemplateLabel};
use observed::{Event, Text};
use templated_uri::PathAndQuery;

use super::events::{RequestAbandoned, RequestComplete, RequestError, RequestTotalAbandoned, RequestTotalComplete, RequestTotalError};
use super::layer::Client;
use super::request::{ClientRequest, RequestEnd};
use crate::Telemetry;
use crate::drop_guard::TelemetryDropGuard;
use crate::shared::{OTHER, known_scheme};

impl<T: RequestHandler> Telemetry<T> {
    /// The [`execute`](layered::Service::execute) of a client middleware.
    pub(crate) fn execute_client<'a>(
        &'a self,
        client: &'a Client,
        input: HttpRequest,
    ) -> impl Future<Output = Result<HttpResponse>> + Send {
        // #[gamma::skip(collection.reverse_array, tag = "equivalent", reason = "`is_interested` gives the same answer in each order of the descriptions.")]
        let descriptions = if client.report_total_duration {
            [
                RequestTotalComplete::DESCRIPTION,
                RequestTotalError::DESCRIPTION,
                RequestTotalAbandoned::DESCRIPTION,
            ]
        } else {
            [
                RequestComplete::DESCRIPTION,
                RequestError::DESCRIPTION,
                RequestAbandoned::DESCRIPTION,
            ]
        };
        if !self.is_interested(&descriptions) {
            return Either::Left(self.inner.execute(input));
        }

        let uri = input.uri();
        let (scheme, default_port) = match uri.scheme_str() {
            Some(scheme) => match known_scheme(scheme) {
                Some((scheme, default_port)) => (Text::from(scheme), Some(default_port)),
                None => (Text::from(Arc::<str>::from(scheme)), None),
            },
            None => (Text::from(OTHER), None),
        };
        let authority = uri.authority().cloned();
        let mut guard = TelemetryDropGuard::new(
            ClientRequest {
                sink: &self.sink,
                client,
                method: input.method().clone(),
                scheme,
                server_port: authority.as_ref().and_then(|authority| authority.port_u16().or(default_port)),
                authority,
                url_template: input.uri_template_label().map(UriTemplateLabel::into_cow),
                path: input
                    .path_and_query()
                    .cloned()
                    .or_else(|| uri.path_and_query().cloned().map(PathAndQuery::from)),
            },
            &self.sink,
            &self.clock,
        );
        // The middleware resumes each panic that it catches and never polls the handler again, so the
        // handler does not need to be unwind safe. The handler can panic in the call too, because
        // `Execute` runs the handler closure in `execute`.
        let response = match catch_unwind(AssertUnwindSafe(|| self.inner.execute(input))) {
            Ok(response) => response,
            Err(payload) => {
                guard.emit(RequestEnd::Panic);
                resume_unwind(payload)
            }
        };
        Either::Right(async move {
            match AssertUnwindSafe(response).catch_unwind().await {
                Ok(Ok(response)) => {
                    guard.emit(RequestEnd::Complete(response.status(), response.version()));
                    Ok(response)
                }
                Ok(Err(error)) => {
                    guard.emit(RequestEnd::Error(&error));
                    Err(error)
                }
                Err(payload) => {
                    guard.emit(RequestEnd::Panic);
                    resume_unwind(payload)
                }
            }
        })
    }
}
