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

let handler = Execute::new(move |_request: HttpRequest| {
    let response = HttpResponse::new(body_builder.empty());
    async move { Ok::<_, HttpError>(response) }
});

let layer = Telemetry::server(sink);
let service = (layer, handler).into_service();
```

## Events

When a request ends, the middleware emits one of these events to an [`observed::Sink`][__link4], except
in the cases listed under
[When the middleware emits no event](#when-the-middleware-emits-no-event).

|Event|Severity|Message|When|
|-----|--------|-------|----|
|`http.server.request.complete`|Info|`HTTP server request completed`|The handler returned a response, with any status.|
|`http.server.request.abandoned`|Warn|`HTTP server request abandoned`|The caller dropped the request future before the handler returned.|

Both events carry the [attributes](#metric-httpserverrequestduration) of the metric
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
|`network.protocol.version`|string|Version of the HTTP protocol. `_OTHER` for a version other than 1.0, 1.1, 2 and 3.|`1.1`; `2`|Always|
|`http.response.status_code`|int|HTTP response status code.|`200`|If the handler returned a response.|
|`error.type`|string|Describes a class of error the request ended with.|`abandoned`|If the caller dropped the request future before the handler returned.|

## When the middleware emits no event

* The handler returns an error or panics. The error or the panic reaches the caller unchanged.
* The request future is dropped while its thread panics.
* There is no destination interested in processing the event further.

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

let service = (Telemetry::server(sink.clone()), handler).into_service();

let response = async { service.execute(request).await }
    .enrich(&sink, Region { region: "westus" })
    .await;
```


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/http_telemetry">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQb5jhta0JV0rUbwjoBka9dQDwbHwIZlDkmysEbluWAwmzhzEthZIOCbmh0dHBfdGVsZW1ldHJ5ZTAuMS4wgmdsYXllcmVkZTAuMy44gmhvYnNlcnZlZGYwLjI5LjA
 [__link0]: https://crates.io/crates/observed/0.29.0
 [__link1]: https://docs.rs/http_telemetry/0.1.0/http_telemetry/?search=Telemetry::server
 [__link2]: https://docs.rs/http_telemetry/0.1.0/http_telemetry/?search=Telemetry
 [__link3]: https://docs.rs/layered/0.3.8/layered/?search=Layer
 [__link4]: https://docs.rs/observed/0.29.0/observed/?search=Sink
