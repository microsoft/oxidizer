<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Http Telemetry Logo" width="96">

# Http Telemetry

[![crate.io](https://img.shields.io/crates/v/http_telemetry.svg)](https://crates.io/crates/http_telemetry)
[![docs.rs](https://docs.rs/http_telemetry/badge.svg)](https://docs.rs/http_telemetry)
[![MSRV](https://img.shields.io/crates/msrv/http_telemetry)](https://crates.io/crates/http_telemetry)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Automatic telemetry collection for incoming and outgoing HTTP requests.

The crate produces telemetry about HTTP requests in the form of [`observed`][__link0] events. A layer
builds the [`Telemetry`][__link1] middleware around a service ([`layered::Layer`][__link2]), and
`(layer, handler).into_service()` wraps the handler in the middleware.

On a server, [`Telemetry::server`][__link3] returns the layer, and the handler is the application. If
the tuple has other layers, put this layer first, so that the middleware also produces
telemetry about the requests that another layer responds to.

```rust
use http_extensions::{HttpBodyBuilder, HttpError, HttpRequest, HttpResponse};
use http_telemetry::Telemetry;
use layered::{Execute, Stack};
use observed::Sink;

let layer = Telemetry::server(sink);
let handler = Execute::new(move |_request: HttpRequest| {
    let response = HttpResponse::new(body_builder.empty());
    async move { Ok::<_, HttpError>(response) }
});

let service = (layer, handler).into_service();
```

On a client, [`Telemetry::client`][__link4] returns the layer, and the handler sends the request. A layer
with [`report_total_duration(true)`][__link5] reports each request
as a whole. Put such a layer outside retries and hedging, and a second layer inside them, so that
the events cover each request and each of its attempts.

```rust
use http_extensions::{HttpBodyBuilder, HttpError, HttpRequest, HttpResponse};
use http_telemetry::Telemetry;
use layered::{Execute, Stack};
use observed::Sink;

let total = Telemetry::client(sink.clone()).report_total_duration(true);
let attempt = Telemetry::client(sink);
let handler = Execute::new(move |_request: HttpRequest| {
    let response = HttpResponse::new(body_builder.empty());
    async move { Ok::<_, HttpError>(response) }
});

let service = (total, attempt, handler).into_service();
```

## Server

### Server events

When a request ends, the middleware emits one of these events to an [`observed::Sink`][__link6], except
in the cases listed under
[When the server middleware emits no event](#when-the-server-middleware-emits-no-event).
[When a request ends](#when-a-request-ends) describes the end of a request whose response has a
body.

|Event|Severity|Message|When|
|-----|--------|-------|----|
|`http.server.request.complete`|Info|`HTTP server request completed`|The handler returned a response, with any status, and the response body ended.|
|`http.server.request.error`|Error|`HTTP server request failed`|The handler or the response body returned an error or panicked.|
|`http.server.request.abandoned`|Warn|`HTTP server request abandoned`|The caller dropped the request future before the handler returned, or the server dropped the response body before its end.|

Each event has the [attributes](#metric-httpserverrequestduration) of the metric
`http.server.request.duration`, and these fields:

|Field|Type|Description|Examples|Presence|
|-----|----|-----------|--------|--------|
|`http.server.request.duration`|double|HTTP request duration in seconds.|`0.012`|Always|
|`http.request.method_original`|string|The method as the client sent it.|`PURGE`|If `http.request.method` is `_OTHER`.|
|`http.route.parameter.<name>`|string|The value of a [configured route parameter][__link7], percent-encoded as in the request, redacted according to its data class.|`1`|If the request matches a route that has the parameter.|
|`http.request.header.<name>`|string|The value of a [configured request header][__link8], redacted according to its data class. The values of repeated header lines are joined with `, `, and bytes that are not valid UTF-8 become `�`.|`0f3a9c`|If the request has the header.|
|`http.response.header.<name>`|string|The value of a [configured response header][__link9], as for a request header.|`120`|If the response has the header.|
|`http.request.body.content`|string|The start of the request body, up to the [size limit][__link10], as text. If it reaches the limit, an incomplete character at its end is left out. Bytes that are not valid UTF-8 become `�`. It holds only the bytes that the handler read, unless the body is in memory. See [Bodies](#bodies).|`{"stars":5}`|If the [request body media types][__link11] cover the `Content-Type` of the request.|
|`http.response.body.content`|string|The start of the response body, up to the [size limit][__link12], as text. If it reaches the limit, an incomplete character at its end is left out. Bytes that are not valid UTF-8 become `�`. Empty for a response without content. See [Bodies](#bodies).|`{"book":"1"}`|If the [response body media types][__link13] cover the `Content-Type` of the response.|

### Server metrics

#### Metric: `http.server.request.duration`

|Name|Instrument Type|Unit (UCUM)|Description|
|----|---------------|-----------|-----------|
|`http.server.request.duration`|Histogram|`s`|Duration of HTTP server requests.|

|Attribute|Type|Description|Examples|Presence|
|---------|----|-----------|--------|--------|
|`http.request.method`|string|HTTP request method. `_OTHER` for any method other than `CONNECT`, `DELETE`, `GET`, `HEAD`, `OPTIONS`, `PATCH`, `POST`, `PUT` and `TRACE`.|`GET`; `POST`; `_OTHER`|Always|
|`url.scheme`|string|The scheme of the request URI. `_OTHER` for a scheme other than `http` and `https`.|`http`; `https`; `_OTHER`|If the request URI has a scheme.|
|`network.protocol.version`|string|Version of the HTTP protocol. `_OTHER` for a version other than 1.0, 1.1, 2 and 3.|`1.1`; `2`|Always|
|`server.address`|string|Host of the server, from the request URI, else from the `Host` header. An IPv6 address keeps its brackets.|`example.com`; `[2001:db8::1]`|If the request URI or the `Host` header names a host.|
|`server.port`|int|Port of the server, from the request URI, else 80 for `http` and 443 for `https`. A request URI without a host takes the port from the `Host` header.|`443`; `8080`|If the port is known.|
|`http.route`|string|The template of the route that the request matches.|`/books/{book}`|If the layer has a [resolver][__link14] and the request matches one of its routes.|
|`http.response.status_code`|int|HTTP response status code.|`200`|If the handler returned a response.|
|`error.type`|string|Describes a class of error the request ended with. An error of the handler or of the response body gives the labels of the error and its sources, joined with dots: the label of each [`HttpError`][__link15] and the kind of each I/O error. A panic of the handler or of the response body gives `panic`. A request future or a response body that is dropped before its end gives `abandoned`. Else, a status from 500 to 599 gives the status code.|`validation`; `io.connection_refused`; `panic`; `503`; `abandoned`|If the request ended with an error, a panic or a drop, or with a status from 500 to 599.|

`server.address` and `server.port` come from the request, so a client that sends new host names
creates new metric series.

The middleware matches each request to a route of its resolver, with the method and the path of
the request. The query does not take part. Give the middleware a resolver with the same routes
as the application, so that both match a request to the same route.

#### Metric: `http.server.active_requests`

|Name|Instrument Type|Unit (UCUM)|Description|
|----|---------------|-----------|-----------|
|`http.server.active_requests`|`UpDownCounter`|`{request}`|Number of active HTTP server requests.|

|Attribute|Type|Description|Examples|Presence|
|---------|----|-----------|--------|--------|
|`http.request.method`|string|HTTP request method, as for `http.server.request.duration`.|`GET`; `POST`; `_OTHER`|Always|
|`url.scheme`|string|The scheme of the request URI, as for `http.server.request.duration`.|`http`; `https`; `_OTHER`|If the request URI has a scheme.|

The middleware adds `1` when it starts a request, before it calls the handler. It adds `-1` when
the request ends, after the event of the request. Each change is also the event
`http.server.active_requests` at Debug, with the message `Active HTTP server requests changed`,
the change under the key `http.server.active_requests`, and the attributes of the metric.

The counter stays one too high when the middleware cannot add the `-1`: if the request future or
the response body is dropped while its thread panics, or if a processor panics while the
middleware emits.

### When a request ends

The middleware wraps the response body, and the request ends when the body ends. The body ends
when one of these occurs:

* A poll of the body returns no frame.
* A poll returns trailers.
* A poll returns data, and then [`is_end_stream`][__link16] of the body
  returns `true`.
* The server drops the body while `is_end_stream` returns `true`.
* Over HTTP/1.0 or HTTP/1.1, and without a `Transfer-Encoding` header, the server drops the body
  after the length that the response declares with a `Content-Length` header, else the exact size
  hint of the body.

These responses have no content, so the middleware does not wrap their body, and their request
ends when the handler returns:

* A response whose body returns `true` from `is_end_stream` when the handler returns.
* A response to `HEAD`.
* A response to `CONNECT` with a status from 200 to 299.
* A response with a status from 100 to 199, 204 or 304.

### Bodies

The middleware logs no body of a direction while the list of media types of that direction is
empty, which is the default. If the list is not empty, the middleware always logs a body whose
`Content-Type` is one of these types:

* A `text` type, such as `text/plain` or `text/csv`.
* An `application` type whose subtype ends with `+json` or `+xml`, such as
  `application/problem+json`.

The media types in the list add more types, such as `application/json`. The list cannot remove
the types above. So with only `application/json` in the list, the middleware also logs
`text/csv` and `application/soap+xml` bodies. ASCII case and the parameters of a `Content-Type`
do not matter.

The middleware does not redact the bodies that it logs, so the logs can contain personal data
from the bodies. It does not log a body whose `Content-Encoding` names a coding other than
`identity`, because the text of compressed bytes cannot be read. To log the bodies that the
server layer of `http_compression` compresses or decompresses, put this layer after that layer
in the tuple.

The middleware copies the bytes of a logged request body while the handler reads the body, so
the events show the bytes that the handler read before the request ended. The middleware does
not read a request body by itself, so it does not delay the handler. A request body whose bytes
are already in memory is copied when the request arrives, also if the handler does not read it.

The middleware copies the bytes of a logged response body while the server reads the body, so
the error and abandoned events show the bytes that the server read before the request ended.

The copy of a body grows only with the bytes that arrive, up to the size limit, so the length
that a client declares does not decide how much memory the middleware uses.

### When the server middleware emits no event

* The URI path of the request matches one of the
  [`excluded_paths`][__link17].
* The request future or the response body is dropped while its thread panics.
* There is no destination interested in processing the event further.

## Client

A client request ends when the handler returns. The middleware does not read or wrap the
response body.

### Client events

When a request ends, the middleware emits one of these events to an [`observed::Sink`][__link18], except
in the cases listed under
[When the client middleware emits no event](#when-the-client-middleware-emits-no-event). A
layer emits the events of an attempt, or, with
[`report_total_duration(true)`][__link19], the events of a whole
request.

|Event|Severity|Message|When|
|-----|--------|-------|----|
|`http.client.request.complete`|Debug|`HTTP request attempt completed`|The handler returned a response, with any status.|
|`http.client.request.error`|Warn|`HTTP request attempt failed`|The handler returned an error or panicked.|
|`http.client.request.abandoned`|Warn|`HTTP request attempt abandoned`|The caller dropped the request future before the handler returned.|
|`http.client.request.total.complete`|Debug|`HTTP request completed`|As `http.client.request.complete`, for a whole request.|
|`http.client.request.total.error`|Warn|`HTTP request failed`|As `http.client.request.error`, for a whole request.|
|`http.client.request.total.abandoned`|Warn|`HTTP request abandoned`|As `http.client.request.abandoned`, for a whole request.|

Each event has the [attributes](#metric-httpclientrequestduration) of its metric,
`http.client.request.duration` or `http.client.request.total_duration`, and these fields:

|Field|Type|Description|Examples|Presence|
|-----|----|-----------|--------|--------|
|`http.client.request.duration`|double|HTTP request duration in seconds.|`0.012`|In the events of an attempt.|
|`http.client.request.total_duration`|double|HTTP request duration in seconds.|`0.012`|In the events of a whole request.|
|`http.request.method_original`|string|The method of the request.|`PURGE`|If `http.request.method` is `_OTHER`.|
|`url.path.redacted`|string|The path and the query of the request URI, redacted by the redactor of each processor. Each part of a [templated][__link20] path is redacted according to its data class. A path without a template is redacted as a whole, with the data class [`Uri::DATA_CLASS`][__link21].|`/books/1`|If the request URI has a path.|
|`exception.message`|string|The message of the error that the handler returned.|`the review has no text`|If the handler returned an error.|

The middleware does not redact `exception.message`, so the logs can contain personal data from
the message of an error.

A layer inside retries or hedging reports each attempt. If a timeout cuts an attempt, or a
hedged attempt loses, the attempt is dropped, so its event is `http.client.request.abandoned`
at Warn. A log processor can skip that event by its name.

### Client metrics

#### Metric: `http.client.request.duration`

|Name|Instrument Type|Unit (UCUM)|Description|
|----|---------------|-----------|-----------|
|`http.client.request.duration`|Histogram|`s`|Duration of HTTP client requests.|

|Attribute|Type|Description|Examples|Presence|
|---------|----|-----------|--------|--------|
|`http.request.method`|string|HTTP request method. `_OTHER` for any method other than `CONNECT`, `DELETE`, `GET`, `HEAD`, `OPTIONS`, `PATCH`, `POST`, `PUT` and `TRACE`.|`GET`; `POST`; `_OTHER`|Always|
|`url.scheme`|string|The scheme of the request URI. `_OTHER` if the URI has no scheme.|`http`; `https`; `ftp`; `_OTHER`|Always|
|`server.address`|string|Host of the request URI. An IPv6 address keeps its brackets.|`example.com`; `[2001:db8::1]`|If the request URI has a host.|
|`server.port`|int|Port of the request URI, else 80 for `http` and 443 for `https`.|`443`; `8080`|If the request URI has a host, and the port is known.|
|`url.template`|string|The template of the request URI: the [`UriTemplateLabel`][__link22] of the request, else the label or the template of its [templated][__link23] path, else `unknown`.|`/books/{book}`; `get_book`; `unknown`|Always|
|`network.protocol.name`|string|The protocol of the response.|`http`|If the handler returned a response.|
|`network.protocol.version`|string|Version of the HTTP protocol of the response. `_OTHER` for a version other than 1.0, 1.1, 2 and 3.|`1.1`; `2`|If the handler returned a response.|
|`http.response.status_code`|int|HTTP response status code.|`200`; `503`|If the handler returned a response.|
|`error.type`|string|Describes a class of error the request ended with. An error of the handler gives the labels of the error and its sources, joined with dots: the label of each [`HttpError`][__link24] and the kind of each I/O error, unless the layer has [its own function][__link25]. A panic of the handler gives `panic`. A request future that is dropped before the handler returned gives `abandoned`. A status never gives `error.type`.|`validation`; `io.connection_refused`; `panic`; `abandoned`|If the request ended with an error, a panic or a drop.|

The `url.template` of a request whose URI is given as text is `unknown`, because its path has no
template. To record a template, build the URI from a [templated][__link26] type, or
add a [`UriTemplateLabel`][__link27] to the extensions of the request.

#### Metric: `http.client.request.total_duration`

|Name|Instrument Type|Unit (UCUM)|Description|
|----|---------------|-----------|-----------|
|`http.client.request.total_duration`|Histogram|`s`|Total duration of HTTP client requests, including all retries and hedged attempts.|

It has the attributes of `http.client.request.duration`.

### When the client middleware emits no event

* The request future is dropped while its thread panics.
* There is no destination interested in processing the event further.

## Panics

If the handler panics, or a response body on the server panics while the server reads a frame
from it, the middleware emits the error event and then resumes the panic with its payload. If a
processor panics while the middleware emits an event, the processor panic reaches the caller,
and the processors after it do not get the event. If this happens while the middleware reports a
panic, the processor panic replaces that panic.

## Enrichment with custom dimensions

To add a dimension to the events, call `execute` inside an `observed` enrichment scope. The
middleware reads the enrichment when `execute` is called, and adds it to the event, also when
the request ends on another task.

```rust
use http_extensions::{HttpBodyBuilder, HttpError, HttpRequest, HttpResponse};
use http_telemetry::Telemetry;
use layered::{Execute, Service, Stack};
use observed::enrichment::EnrichFutureExt;
use observed::{Enrichment, Sink};

#[derive(Enrichment)]
struct Region {
    #[dimension(log = "cloud.region", metric = "cloud.region")]
    #[unredacted]
    region: &'static str,
}

let service = (layer, handler).into_service();

let response = async { service.execute(request).await }
    .enrich(&sink, Region { region: "westus" })
    .await;
```


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/http_telemetry">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbbIshYuIPZVgb6p2NB-Q279sbZ_sXjSyxFJwb_9VU6p2dBhFhZIaDaWh0dHAtYm9keWUxLjEuMGlodHRwX2JvZHmCb2h0dHBfZXh0ZW5zaW9uc2YwLjE2LjCCbmh0dHBfdGVsZW1ldHJ5ZTAuMi4wgmdsYXllcmVkZjAuMy4xMIJob2JzZXJ2ZWRmMC4zMS4wgm10ZW1wbGF0ZWRfdXJpZTAuNy4y
 [__link0]: https://crates.io/crates/observed/0.31.0
 [__link1]: https://docs.rs/http_telemetry/0.2.0/http_telemetry/?search=Telemetry
 [__link10]: https://docs.rs/http_telemetry/0.2.0/http_telemetry/?search=TelemetryLayer::body_size_limit
 [__link11]: https://docs.rs/http_telemetry/0.2.0/http_telemetry/?search=TelemetryLayer::request_body_media_types
 [__link12]: https://docs.rs/http_telemetry/0.2.0/http_telemetry/?search=TelemetryLayer::body_size_limit
 [__link13]: https://docs.rs/http_telemetry/0.2.0/http_telemetry/?search=TelemetryLayer::response_body_media_types
 [__link14]: https://docs.rs/http_telemetry/0.2.0/http_telemetry/?search=TelemetryLayer::route_resolver
 [__link15]: https://docs.rs/http_extensions/0.16.0/http_extensions/?search=HttpError
 [__link16]: https://docs.rs/http-body/1.1.0/http_body/?search=Body::is_end_stream
 [__link17]: https://docs.rs/http_telemetry/0.2.0/http_telemetry/?search=TelemetryLayer::excluded_paths
 [__link18]: https://docs.rs/observed/0.31.0/observed/?search=Sink
 [__link19]: https://docs.rs/http_telemetry/0.2.0/http_telemetry/?search=TelemetryLayer::report_total_duration
 [__link2]: https://docs.rs/layered/0.3.10/layered/?search=Layer
 [__link20]: https://docs.rs/templated_uri/0.7.2/templated_uri/?search=templated
 [__link21]: https://docs.rs/templated_uri/0.7.2/templated_uri/?search=Uri::DATA_CLASS
 [__link22]: https://docs.rs/http_extensions/0.16.0/http_extensions/?search=UriTemplateLabel
 [__link23]: https://docs.rs/templated_uri/0.7.2/templated_uri/?search=templated
 [__link24]: https://docs.rs/http_extensions/0.16.0/http_extensions/?search=HttpError
 [__link25]: https://docs.rs/http_telemetry/0.2.0/http_telemetry/?search=TelemetryLayer::error_labels_with
 [__link26]: https://docs.rs/templated_uri/0.7.2/templated_uri/?search=templated
 [__link27]: https://docs.rs/http_extensions/0.16.0/http_extensions/?search=UriTemplateLabel
 [__link3]: https://docs.rs/http_telemetry/0.2.0/http_telemetry/?search=Telemetry::server
 [__link4]: https://docs.rs/http_telemetry/0.2.0/http_telemetry/?search=Telemetry::client
 [__link5]: https://docs.rs/http_telemetry/0.2.0/http_telemetry/?search=TelemetryLayer::report_total_duration
 [__link6]: https://docs.rs/observed/0.31.0/observed/?search=Sink
 [__link7]: https://docs.rs/http_telemetry/0.2.0/http_telemetry/?search=TelemetryLayer::route_parameter_data_classes
 [__link8]: https://docs.rs/http_telemetry/0.2.0/http_telemetry/?search=TelemetryLayer::request_header_data_classes
 [__link9]: https://docs.rs/http_telemetry/0.2.0/http_telemetry/?search=TelemetryLayer::response_header_data_classes
