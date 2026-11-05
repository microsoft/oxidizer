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

Automatic telemetry collection for incoming HTTP requests.

The crate produces telemetry about incoming HTTP requests in the form of [`observed`][__link0] events.
[`Telemetry::server`][__link1] returns a layer, which builds the [`Telemetry`][__link2] middleware around a
service ([`layered::Layer`][__link3]). `(layer, handler).into_service()` wraps the handler in the
middleware. If the tuple has other layers, put this layer first, so that the middleware also
produces telemetry about the requests that another layer responds to.

```rust
use http_extensions::{HttpBodyBuilder, HttpError, HttpRequest, HttpResponse};
use http_telemetry::Telemetry;
use layered::{Execute, Stack};
use observed::Sink;

let layer = Telemetry::server(sink, body_builder.clone());
let handler = Execute::new(move |_request: HttpRequest| {
    let response = HttpResponse::new(body_builder.empty());
    async move { Ok::<_, HttpError>(response) }
});

let service = (layer, handler).into_service();
```

## Events

When a request ends, the middleware emits one of these events to an [`observed::Sink`][__link4], except
in the cases listed under
[When the middleware emits no event](#when-the-middleware-emits-no-event).
[When a request ends](#when-a-request-ends) describes the end of a request whose response has a
body.

|Event|Severity|Message|When|
|-----|--------|-------|----|
|`http.server.request.complete`|Info|`HTTP server request completed`|The handler returned a response, with any status, and the response body ended.|
|`http.server.request.error`|Error|`HTTP server request failed`|The handler or the response body returned an error or panicked.|
|`http.server.request.abandoned`|Warn|`HTTP server request abandoned`|The caller dropped the request future before the handler returned, or the server dropped the response body before its end.|

Each event carries the [attributes](#metric-httpserverrequestduration) of the metric
`http.server.request.duration`, and these fields:

|Field|Type|Description|Examples|Presence|
|-----|----|-----------|--------|--------|
|`http.server.request.duration`|int|HTTP request duration in milliseconds.|`12`|Always|
|`http.request.method_original`|string|The method as the client sent it.|`PURGE`|If `http.request.method` is `_OTHER`.|

## Metrics

### Metric: `http.server.request.duration`

|Name|Instrument Type|Unit (UCUM)|Description|
|----|---------------|-----------|-----------|
|`http.server.request.duration`|Histogram|`ms`|Duration of HTTP server requests.|

|Attribute|Type|Description|Examples|Presence|
|---------|----|-----------|--------|--------|
|`http.request.method`|string|HTTP request method. `_OTHER` for any method other than `CONNECT`, `DELETE`, `GET`, `HEAD`, `OPTIONS`, `PATCH`, `POST`, `PUT` and `TRACE`.|`GET`; `POST`; `_OTHER`|Always|
|`url.scheme`|string|The scheme of the request URI. `_OTHER` for a scheme other than `http` and `https`.|`http`; `https`; `_OTHER`|If the request URI has a scheme.|
|`network.protocol.version`|string|Version of the HTTP protocol. `_OTHER` for a version other than 1.0, 1.1, 2 and 3.|`1.1`; `2`|Always|
|`server.address`|string|Host of the server, from the request URI, else from the `Host` header. An IPv6 address keeps its brackets.|`example.com`; `[2001:db8::1]`|If the request URI or the `Host` header names a host.|
|`server.port`|int|Port of the server, from the request URI, else 80 for `http` and 443 for `https`. A request URI without a host takes the port from the `Host` header.|`443`; `8080`|If the port is known.|
|`http.response.status_code`|int|HTTP response status code.|`200`|If the handler returned a response.|
|`error.type`|string|Describes a class of error the request ended with. An error of the handler or of the response body gives the labels of the error and its sources, joined with dots: the label of each [`HttpError`][__link5] and the kind of each I/O error. A panic of the handler or of the response body gives `panic`. A request future or a response body that is dropped before its end gives `abandoned`. Else, a status from 500 to 599 gives the status code.|`validation`; `io.connection_refused`; `panic`; `503`; `abandoned`|If the request ended with an error, a panic or a drop, or with a status from 500 to 599.|

`server.address` and `server.port` come from the request, so a client that sends new host names
creates new metric series.

### Metric: `http.server.active_requests`

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

## When a request ends

The middleware wraps the response body, and the request ends when the body ends. The body ends
when one of these occurs:

* A poll of the body returns no frame.
* A poll returns trailers.
* A poll returns data, and then [`is_end_stream`][__link6] of the body
  returns `true`.
* The server drops the body while `is_end_stream` returns `true`.
* Over HTTP/1.0 or HTTP/1.1, the server drops the body after the length that the response
  declares with a `Content-Length` header, else the exact size hint of the
  body.

These responses have no content, so the middleware does not wrap their body, and their request
ends when the handler returns:

* A response whose body returns `true` from `is_end_stream` when the handler returns.
* A response to `HEAD`.
* A response to `CONNECT` with a status from 200 to 299.
* A response with a status from 100 to 199, 204 or 304.

## When the middleware emits no event

* The URI path of the request starts with one of the
  [`excluded_paths`][__link7].
* The request future or the response body is dropped while its thread panics.
* There is no destination interested in processing the event further.

## Panics

If the handler panics, or the response body panics while the server reads a frame from it, the
middleware emits the error event and then resumes the panic with its payload. If a processor
panics while the middleware emits an event, the processor panic reaches the caller, and the
processors after it do not get the event. If this happens while the middleware reports a panic,
the processor panic replaces that panic.

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

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbTJHIV4hMJjkbyRVPVLTw5DwbvEgmKi68o4AbrciskOSQDXxhZIWDaWh0dHAtYm9keWUxLjEuMGlodHRwX2JvZHmCb2h0dHBfZXh0ZW5zaW9uc2YwLjE0LjCCbmh0dHBfdGVsZW1ldHJ5ZTAuMS4wgmdsYXllcmVkZTAuMy45gmhvYnNlcnZlZGYwLjMwLjA
 [__link0]: https://crates.io/crates/observed/0.30.0
 [__link1]: https://docs.rs/http_telemetry/0.1.0/http_telemetry/?search=Telemetry::server
 [__link2]: https://docs.rs/http_telemetry/0.1.0/http_telemetry/?search=Telemetry
 [__link3]: https://docs.rs/layered/0.3.9/layered/?search=Layer
 [__link4]: https://docs.rs/observed/0.30.0/observed/?search=Sink
 [__link5]: https://docs.rs/http_extensions/0.14.0/http_extensions/?search=HttpError
 [__link6]: https://docs.rs/http-body/1.1.0/http_body/?search=Body::is_end_stream
 [__link7]: https://docs.rs/http_telemetry/0.1.0/http_telemetry/?search=TelemetryLayer::excluded_paths
