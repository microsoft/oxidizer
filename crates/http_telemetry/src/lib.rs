// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![forbid(unsafe_code)]

//! Automatic telemetry collection for incoming and outgoing HTTP requests.
//!
//! The crate produces telemetry about HTTP requests in the form of [`observed`] events. A layer
//! builds the [`Telemetry`] middleware around a service ([`layered::Layer`]), and
//! `(layer, handler).into_service()` wraps the handler in the middleware.
//!
//! On a server, [`Telemetry::server`] returns the layer, and the handler is the application. If
//! the tuple has other layers, put this layer first, so that the middleware also produces
//! telemetry about the requests that another layer responds to.
//!
//! ```
//! use http_extensions::{HttpBodyBuilder, HttpError, HttpRequest, HttpResponse};
//! use http_telemetry::Telemetry;
//! use layered::{Execute, Stack};
//! use observed::Sink;
//!
//! # fn build(sink: Sink, body_builder: HttpBodyBuilder) {
//! let layer = Telemetry::server(sink);
//! let handler = Execute::new(move |_request: HttpRequest| {
//!     let response = HttpResponse::new(body_builder.empty());
//!     async move { Ok::<_, HttpError>(response) }
//! });
//!
//! let service = (layer, handler).into_service();
//! # let _ = service;
//! # }
//! ```
//!
//! On a client, [`Telemetry::client`] returns the layer, and the handler sends the request. A layer
//! with [`report_total_duration(true)`](TelemetryLayer::report_total_duration) reports each request
//! as a whole. Put such a layer outside retries and hedging, and a second layer inside them, so that
//! the events cover each request and each of its attempts.
//!
//! ```
//! use http_extensions::{HttpBodyBuilder, HttpError, HttpRequest, HttpResponse};
//! use http_telemetry::Telemetry;
//! use layered::{Execute, Stack};
//! use observed::Sink;
//!
//! # fn build(sink: Sink, body_builder: HttpBodyBuilder) {
//! let total = Telemetry::client(sink.clone()).report_total_duration(true);
//! let attempt = Telemetry::client(sink);
//! let handler = Execute::new(move |_request: HttpRequest| {
//!     let response = HttpResponse::new(body_builder.empty());
//!     async move { Ok::<_, HttpError>(response) }
//! });
//!
//! let service = (total, attempt, handler).into_service();
//! # let _ = service;
//! # }
//! ```
//!
//! # Server
//!
//! ## Server events
//!
//! When a request ends, the middleware emits one of these events to an [`observed::Sink`], except
//! in the cases listed under
//! [When the server middleware emits no event](#when-the-server-middleware-emits-no-event).
//! [When a request ends](#when-a-request-ends) describes the end of a request whose response has a
//! body.
//!
//! | Event | Severity | Message | When |
//! |---|---|---|---|
//! | `http.server.request.complete` | Info | `HTTP server request completed` | The handler returned a response, with any status, and the response body ended. |
//! | `http.server.request.error` | Error | `HTTP server request failed` | The handler or the response body returned an error or panicked. |
//! | `http.server.request.abandoned` | Warn | `HTTP server request abandoned` | The caller dropped the request future before the handler returned, or the server dropped the response body before its end. |
//!
//! Each event has the [attributes](#metric-httpserverrequestduration) of the metric
//! `http.server.request.duration`, and these fields:
//!
//! | Field | Type | Description | Examples | Presence |
//! |---|---|---|---|---|
//! | `http.server.request.duration` | double | HTTP request duration in seconds. | `0.012` | Always |
//! | `http.request.method_original` | string | The method as the client sent it. | `PURGE` | If `http.request.method` is `_OTHER`. |
//! | `http.route.parameter.<name>` | string | The value of a [configured route parameter](TelemetryLayer::route_parameter_data_classes), percent-encoded as in the request, redacted according to its data class. | `1` | If the request matches a route that has the parameter. |
//! | `http.request.header.<name>` | string | The value of a [configured request header](TelemetryLayer), redacted according to its data class. The values of repeated header lines are joined with `, `, and bytes that are not valid UTF-8 become `�`. | `0f3a9c` | If the request has the header. |
//! | `http.response.header.<name>` | string | The value of a [configured response header](TelemetryLayer), as for a request header. | `120` | If the response has the header. |
//! | `http.request.body.content` | string | The start of the request body, up to the [size limit](TelemetryLayer), as text. If it reaches the limit, an incomplete character at its end is left out. Bytes that are not valid UTF-8 become `�`. It holds only the bytes that the handler read, unless the body is in memory. See [Bodies](#bodies). | `{"stars":5}` | If the [request body media types](TelemetryLayer) cover the `Content-Type` of the request. |
//! | `http.response.body.content` | string | The start of the response body, up to the [size limit](TelemetryLayer), as text. If it reaches the limit, an incomplete character at its end is left out. Bytes that are not valid UTF-8 become `�`. Empty for a response without content. See [Bodies](#bodies). | `{"book":"1"}` | If the [response body media types](TelemetryLayer) cover the `Content-Type` of the response. |
//!
//! ## Server metrics
//!
//! ### Metric: `http.server.request.duration`
//!
//! | Name | Instrument Type | Unit (UCUM) | Description |
//! |---|---|---|---|
//! | `http.server.request.duration` | Histogram | `s` | Duration of HTTP server requests. |
//!
//! Its bucket boundaries are 0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1, 2.5, 5, 7.5 and
//! 10 seconds, as the OpenTelemetry HTTP metrics conventions give.
//!
//! | Attribute | Type | Description | Examples | Presence |
//! |---|---|---|---|---|
//! | `http.request.method` | string | HTTP request method. `_OTHER` for any method other than `CONNECT`, `DELETE`, `GET`, `HEAD`, `OPTIONS`, `PATCH`, `POST`, `PUT` and `TRACE`. | `GET`; `POST`; `_OTHER` | Always |
//! | `url.scheme` | string | The scheme of the request URI. `_OTHER` for a scheme other than `http` and `https`. | `http`; `https`; `_OTHER` | If the request URI has a scheme. |
//! | `network.protocol.version` | string | Version of the HTTP protocol. `_OTHER` for a version other than 1.0, 1.1, 2 and 3. | `1.1`; `2` | Always |
//! | `server.address` | string | Host of the server, from the request URI, else from the `Host` header. An IPv6 address keeps its brackets. | `example.com`; `[2001:db8::1]` | If the request URI or the `Host` header names a host. |
//! | `server.port` | int | Port of the server, from the request URI, else 80 for `http` and 443 for `https`. A request URI without a host takes the port from the `Host` header. | `443`; `8080` | If the port is known. |
//! | `http.route` | string | The template of the route that the request matches. | `/books/{book}` | If the layer has a [resolver](TelemetryLayer::route_resolver) and the request matches one of its routes. |
//! | `http.response.status_code` | int | HTTP response status code. | `200` | If the handler returned a response. |
//! | `error.type` | string | Describes a class of error the request ended with. An error of the handler or of the response body gives the labels of the error and its sources, joined with dots: the label of each [`HttpError`](http_extensions::HttpError) and the kind of each I/O error. A panic of the handler or of the response body gives `panic`. A request future or a response body that is dropped before its end gives `abandoned`. Else, a status from 500 to 599 gives the status code. | `validation`; `io.connection_refused`; `panic`; `503`; `abandoned` | If the request ended with an error, a panic or a drop, or with a status from 500 to 599. |
//!
//! `server.address` and `server.port` come from the request, so a client that sends new host names
//! creates new metric series.
//!
//! The middleware matches each request to a route of its resolver, with the method and the path of
//! the request. The query does not take part. Give the middleware a resolver with the same routes
//! as the application, so that both match a request to the same route.
//!
//! ### Metric: `http.server.active_requests`
//!
//! | Name | Instrument Type | Unit (UCUM) | Description |
//! |---|---|---|---|
//! | `http.server.active_requests` | `UpDownCounter` | `{request}` | Number of active HTTP server requests. |
//!
//! | Attribute | Type | Description | Examples | Presence |
//! |---|---|---|---|---|
//! | `http.request.method` | string | HTTP request method, as for `http.server.request.duration`. | `GET`; `POST`; `_OTHER` | Always |
//! | `url.scheme` | string | The scheme of the request URI, as for `http.server.request.duration`. | `http`; `https`; `_OTHER` | If the request URI has a scheme. |
//!
//! The middleware adds `1` when it starts a request, before it calls the handler. It adds `-1` when
//! the request ends, after the event of the request. Each change is also the event
//! `http.server.active_requests` at Debug, with the message `Active HTTP server requests changed`,
//! the change under the key `http.server.active_requests`, and the attributes of the metric.
//!
//! The counter stays one too high when the middleware cannot add the `-1`: if the request future or
//! the response body is dropped while its thread panics, or if a processor panics while the
//! middleware emits.
//!
//! ## When a request ends
//!
//! The middleware wraps the response body, and the request ends when the body ends. The body ends
//! when one of these occurs:
//!
//! - A poll of the body returns no frame.
//! - A poll returns trailers.
//! - A poll returns data, and then [`is_end_stream`](http_body::Body::is_end_stream) of the body
//!   returns `true`.
//! - The server drops the body while `is_end_stream` returns `true`.
//! - Over HTTP/1.0 or HTTP/1.1, and without a `Transfer-Encoding` header, the server drops the body
//!   after the length that the response declares with a `Content-Length` header, else the exact size
//!   hint of the body.
//!
//! These responses have no content, so the middleware does not wrap their body, and their request
//! ends when the handler returns:
//!
//! - A response whose body returns `true` from `is_end_stream` when the handler returns.
//! - A response to `HEAD`.
//! - A response to `CONNECT` with a status from 200 to 299.
//! - A response with a status from 100 to 199, 204 or 304.
//!
//! ## Bodies
//!
//! The middleware logs no body of a direction while the list of media types of that direction is
//! empty, which is the default. If the list is not empty, the middleware always logs a body whose
//! `Content-Type` is one of these types:
//!
//! - A `text` type, such as `text/plain` or `text/csv`.
//! - An `application` type whose subtype ends with `+json` or `+xml`, such as
//!   `application/problem+json`.
//!
//! The media types in the list add more types, such as `application/json`. The list cannot remove
//! the types above. So with only `application/json` in the list, the middleware also logs
//! `text/csv` and `application/soap+xml` bodies. ASCII case and the parameters of a `Content-Type`
//! do not matter.
//!
//! The middleware does not redact the bodies that it logs, so the logs can contain personal data
//! from the bodies. It does not log a body whose `Content-Encoding` names a coding other than
//! `identity`, because the text of compressed bytes cannot be read. To log the bodies that the
//! server layer of `http_compression` compresses or decompresses, put this layer after that layer
//! in the tuple. To log the response bodies that its client layer decompresses, put this layer
//! before that layer.
//!
//! The middleware copies the bytes of a logged request body while the handler reads the body, so
//! the events show the bytes that the handler read before the request ended. The middleware does
//! not read a request body by itself, so it does not delay the handler. A request body whose bytes
//! are already in memory is copied when the request arrives, also if the handler does not read it.
//!
//! The server middleware copies the bytes of a logged response body while the server reads the
//! body, so the error and abandoned events show the bytes that the server read before the request
//! ended.
//!
//! The client middleware copies the bytes of a logged response body while the caller reads the
//! body. It emits the complete event when the caller drops the body, and the event shows the bytes
//! that the caller read. The duration still ends when the handler returns. An error of the body,
//! or a drop before its end, does not change the complete event. A response body whose bytes are
//! already in memory, or that is at its end, is copied when the handler returns, and the event
//! comes then. Methods that read a whole body, such as
//! [`HttpBody::into_bytes`](http_extensions::HttpBody::into_bytes), drop it at its end.
//!
//! The middleware copies the bytes of a streamed body into room that it takes from a
//! [`GlobalPool`](bytesbuf::mem::GlobalPool) when the first bytes arrive. All layers share the pool.
//! The room holds the whole size limit, so a length that the other side declares does not decide how
//! much memory the middleware uses. The middleware gives the room back when it drops the buffer. The
//! pool keeps the memory that it hands out until the process ends, so it holds room for as many
//! bodies as were once logged at the same time.
//!
//! ## When the server middleware emits no event
//!
//! - The URI path of the request matches one of the
//!   [`excluded_paths`](TelemetryLayer::excluded_paths). The request is also left out of both metrics.
//! - The request future or the response body is dropped while its thread panics.
//! - There is no destination interested in processing the event further.
//!
//! # Client
//!
//! A client request ends when the handler returns. The middleware does not read the response body.
//! If it logs the response body, it wraps the body and emits the complete event when the caller
//! drops the body. See [Bodies](#bodies).
//!
//! In [`fetch`](https://docs.rs/fetch), a custom pipeline emits request events only if it stacks the layer of
//! `PipelineContext::client_telemetry()`.
//!
//! ## Client events
//!
//! When a request ends, the middleware emits one of these events to an [`observed::Sink`], except
//! in the cases listed under
//! [When the client middleware emits no event](#when-the-client-middleware-emits-no-event). A
//! layer emits the events of an attempt, or, with
//! [`report_total_duration(true)`](TelemetryLayer::report_total_duration), the events of a whole
//! request.
//!
//! | Event | Severity | Message | When |
//! |---|---|---|---|
//! | `http.client.request.complete` | Debug | `HTTP request attempt completed` | The handler returned a response, with any status. See [Bodies](#bodies) for a logged response body. |
//! | `http.client.request.error` | Warn | `HTTP request attempt failed` | The handler returned an error or panicked. |
//! | `http.client.request.abandoned` | Warn | `HTTP request attempt abandoned` | The caller dropped the request future before the handler returned. |
//! | `http.client.request.total.complete` | Debug | `HTTP request completed` | As `http.client.request.complete`, for a whole request. |
//! | `http.client.request.total.error` | Warn | `HTTP request failed` | As `http.client.request.error`, for a whole request. |
//! | `http.client.request.total.abandoned` | Warn | `HTTP request abandoned` | As `http.client.request.abandoned`, for a whole request. |
//!
//! Each event has the [attributes](#metric-httpclientrequestduration) of its metric,
//! `http.client.request.duration` or `http.client.request.total_duration`, and these fields:
//!
//! | Field | Type | Description | Examples | Presence |
//! |---|---|---|---|---|
//! | `http.client.request.duration` | double | HTTP request duration in seconds. | `0.012` | In the events of an attempt. |
//! | `http.client.request.total_duration` | double | HTTP request duration in seconds. | `0.012` | In the events of a whole request. |
//! | `http.request.method_original` | string | The method of the request. | `PURGE` | If `http.request.method` is `_OTHER`. |
//! | `url.path.redacted` | string | The path and the query of the request URI, redacted by the redactor of each processor. Each part of a [templated](templated_uri::templated) path is redacted according to its data class. A path without a template is redacted as a whole, with the data class [`Uri::DATA_CLASS`](templated_uri::Uri::DATA_CLASS). | `/books/1` | If the request URI has a path. |
//! | `exception.message` | string | The message of the error that the handler returned. | `the review has no text` | If the handler returned an error. |
//! | `http.request.header.<name>` | string | The value of a [configured request header](TelemetryLayer), as on the server. | `0f3a9c` | If the request has the header when it reaches the layer. |
//! | `http.response.header.<name>` | string | The value of a [configured response header](TelemetryLayer), as on the server. | `120` | If the handler returned a response with the header. |
//! | `http.request.body.content` | string | The start of the request body, as on the server. | `{"stars":5}` | If the [request body media types](TelemetryLayer) cover the `Content-Type` of the request. |
//! | `http.response.body.content` | string | The start of the response body, up to the size limit, as text, as on the server. It holds only the bytes that the caller read before it dropped the body, unless the body is in memory. See [Bodies](#bodies). | `{"book":"1"}` | In the complete events, if the [response body media types](TelemetryLayer) cover the `Content-Type` of the response. |
//!
//! The middleware does not redact `exception.message`, so the logs can contain personal data from
//! the message of an error.
//!
//! A layer inside retries or hedging reports each attempt. If a timeout cuts an attempt, or a
//! hedged attempt loses, the attempt is dropped, so its event is `http.client.request.abandoned`
//! at Warn. A log processor can skip that event by its name.
//!
//! ## Client metrics
//!
//! ### Metric: `http.client.request.duration`
//!
//! | Name | Instrument Type | Unit (UCUM) | Description |
//! |---|---|---|---|
//! | `http.client.request.duration` | Histogram | `s` | Duration of HTTP client requests. |
//!
//! Its bucket boundaries are those of `http.server.request.duration`.
//!
//! | Attribute | Type | Description | Examples | Presence |
//! |---|---|---|---|---|
//! | `http.request.method` | string | HTTP request method. `_OTHER` for any method other than `CONNECT`, `DELETE`, `GET`, `HEAD`, `OPTIONS`, `PATCH`, `POST`, `PUT` and `TRACE`. | `GET`; `POST`; `_OTHER` | Always |
//! | `url.scheme` | string | The scheme of the request URI. `_OTHER` if the URI has no scheme. | `http`; `https`; `ftp`; `_OTHER` | Always |
//! | `server.address` | string | Host of the request URI. An IPv6 address keeps its brackets. | `example.com`; `[2001:db8::1]` | If the request URI has a host. |
//! | `server.port` | int | Port of the request URI, else 80 for `http` and 443 for `https`. | `443`; `8080` | If the request URI has a host, and the port is known. |
//! | `url.template` | string | The template of the request URI: the [`UriTemplateLabel`](http_extensions::UriTemplateLabel) of the request, else the label or the template of its [templated](templated_uri::templated) path, else `unknown`. | `/books/{book}`; `get_book`; `unknown` | Always |
//! | `network.protocol.name` | string | The protocol of the response. | `http` | If the handler returned a response. |
//! | `network.protocol.version` | string | Version of the HTTP protocol of the response. `_OTHER` for a version other than 1.0, 1.1, 2 and 3. | `1.1`; `2` | If the handler returned a response. |
//! | `http.response.status_code` | int | HTTP response status code. | `200`; `503` | If the handler returned a response. |
//! | `error.type` | string | Describes a class of error the request ended with. An error of the handler gives the labels of the error and its sources, joined with dots: the label of each [`HttpError`](http_extensions::HttpError) and the kind of each I/O error, unless the layer has [its own function](TelemetryLayer::error_labels_with). A panic of the handler gives `panic`. A request future that is dropped before the handler returned gives `abandoned`. A status never gives `error.type`. | `validation`; `io.connection_refused`; `panic`; `abandoned` | If the request ended with an error, a panic or a drop. |
//!
//! The `url.template` of a request whose URI is given as text is `unknown`, because its path has no
//! template. To record a template, build the URI from a [templated](templated_uri::templated) type, or
//! add a [`UriTemplateLabel`](http_extensions::UriTemplateLabel) to the extensions of the request.
//!
//! ### Metric: `http.client.request.total_duration`
//!
//! | Name | Instrument Type | Unit (UCUM) | Description |
//! |---|---|---|---|
//! | `http.client.request.total_duration` | Histogram | `s` | Total duration of HTTP client requests, including all retries and hedged attempts. |
//!
//! It has the attributes and the bucket boundaries of `http.client.request.duration`.
//!
//! ## When the client middleware emits no event
//!
//! - The request future, or a response body that the middleware logs, is dropped while its thread
//!   panics.
//! - There is no destination interested in processing the event further.
//!
//! # Panics
//!
//! If the handler panics, or a response body on the server panics while the server reads a frame
//! from it, the middleware emits the error event and then resumes the panic with its payload. If a
//! processor panics while the middleware emits an event, the processor panic reaches the caller,
//! the processors after it do not get the event, and the middleware emits no later event of the
//! request. If this happens while the middleware reports a panic, the processor panic replaces that
//! panic.
//!
//! # Enrichment with custom dimensions
//!
//! To add a dimension to the events, call `execute` inside an `observed` enrichment scope. The
//! middleware reads the enrichment when `execute` is called, and adds it to the event, also when
//! the request ends on another task.
//!
//! ```
//! use http_extensions::{HttpBodyBuilder, HttpError, HttpRequest, HttpResponse};
//! use http_telemetry::Telemetry;
//! use layered::{Execute, Service, Stack};
//! use observed::enrichment::EnrichFutureExt;
//! use observed::{Enrichment, Sink};
//!
//! #[derive(Enrichment)]
//! struct Region {
//!     #[dimension(log = "cloud.region", metric = "cloud.region")]
//!     #[unredacted]
//!     region: &'static str,
//! }
//!
//! # async fn serve(sink: Sink, body_builder: HttpBodyBuilder, request: HttpRequest) {
//! # let layer = Telemetry::server(sink.clone());
//! # let handler = Execute::new(move |_request: HttpRequest| {
//! #     let response = HttpResponse::new(body_builder.empty());
//! #     async move { Ok::<_, HttpError>(response) }
//! # });
//! let service = (layer, handler).into_service();
//!
//! let response = async { service.execute(request).await }
//!     .enrich(&sink, Region { region: "westus" })
//!     .await;
//! # let _ = response;
//! # }
//! ```

mod body_buffer;
mod client;
mod drop_guard;
mod header_fields;
mod interner;
mod media_types;
mod request_body;
mod server;
mod shared;
mod telemetry;

pub use client::Client;
pub use media_types::MediaTypeError;
pub use server::Server;
pub use telemetry::{Telemetry, TelemetryLayer};
