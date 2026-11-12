// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runs a hyper server with the server middleware, and prints the events that it emits.
//!
//! The server takes HTTP/1.1 and HTTP/2 on one local port. The application routes each request
//! with a `routerama` resolver, and the middleware matches each request to a route with the same
//! resolver. A client sends five requests over each protocol:
//!
//! - `GET /books/1`: the server responds with a short JSON body. The middleware emits the complete
//!   event when the body ends. The event shows the route as `http.route=/books/{book}`. The
//!   layer gives the route parameter `book` a data class that the printer shows unchanged:
//!   `http.route.parameter.book=1`.
//! - `GET /missing`: no route matches, so the server responds with 404 and no body. The middleware
//!   emits the complete event when the handler returns. The event has no `http.route`.
//! - `GET /books/1/chapters`: the server sends one chunk of the body and then waits. The client
//!   reads the chunk and hangs up, so the middleware emits the abandoned event.
//! - `GET /books/1/pages`: the server sends one chunk of the body, and then the body fails. The
//!   middleware emits the error event.
//! - `GET /health`: the server responds with 200. The layer excludes the path, so the middleware
//!   emits no event for it.
//!
//! Each request has an `x-request-id` header. The layer gives the header a data class that the
//! printer shows unchanged, so each event shows its value as `http.request.header.x-request-id`.
//!
//! The middleware also emits the `1` and the `-1` of `http.server.active_requests` for each
//! request that it does not exclude. The client waits for that `-1` before it prints its line and
//! sends the next request. Each event shows the host and the port of the server: from the `Host`
//! header over HTTP/1.1, and from the URI over HTTP/2.
//!
//! Run the example with this command:
//! ```sh
//! cargo run -p http_telemetry --example hyper_server
//! ```

#[path = "support/print.rs"]
mod print;

use std::io;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use bytesbuf::BytesView;
use data_privacy::{DataClass, RedactionEngine};
use futures::channel::mpsc::{self, UnboundedSender};
use futures::{StreamExt, stream};
use http::header::{CONTENT_TYPE, HOST};
use http::{HeaderName, Request, Response, StatusCode};
use http_body_util::{BodyExt, Empty};
use http_extensions::{HttpBodyBuilder, HttpError, HttpRequest};
use http_telemetry::Telemetry;
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use layered::{Execute, Service, Stack};
use observed::metadata::{EventDescription, FieldDescriptor};
use observed::processing::{EventProcessor, EventView, FieldValueFn};
use observed::{FlushError, Sink, Value};
use print::PrintProcessor;
use templated_uri::BasePath;
use tokio::net::{TcpListener, TcpStream};

/// The time after which the example stops. `just anvil-examples` stops an example after 30
/// seconds.
const TIMEOUT: Duration = Duration::from_secs(10);

/// The paths that the client requests over each protocol.
const PATHS: [&str; 5] = ["/books/1", "/missing", "/books/1/chapters", "/books/1/pages", HEALTH_PATH];

/// The path that the layer excludes, so that the middleware emits no event for it.
const HEALTH_PATH: &str = "/health";

/// The path whose response the client reads in part, before it hangs up.
const HANG_UP_PATH: &str = "/books/1/chapters";

/// The name of the event that the middleware emits for each change of the active requests.
const ACTIVE_REQUESTS: &str = "http.server.active_requests";

/// The header that the client sends with each request, and that the events show.
const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// The data class whose values the printer shows unchanged.
const PUBLIC: DataClass = DataClass::new("example", "public");

/// The routes of the application.
#[routerama::resolver]
enum BookRoute<'p> {
    #[route(GET, "/books/{book}")]
    GetBook { book: &'p str },

    #[route(GET, "/books/{book}/chapters")]
    GetChapters { book: &'p str },

    #[route(GET, "/books/{book}/pages")]
    GetPages { book: &'p str },

    #[route(GET, "/health")]
    Health,
}

/// Sends a signal at the end of each request, which is its `-1` of `http.server.active_requests`.
struct RequestEndProcessor {
    engine: RedactionEngine,
    sender: UnboundedSender<()>,
}

impl EventProcessor for RequestEndProcessor {
    fn is_interested(&self, description: &EventDescription) -> bool {
        description.name() == ACTIVE_REQUESTS
    }

    fn process(&self, event: &EventView<'_>) {
        let _ = event.visit_fields(&mut |descriptor: &FieldDescriptor, value: &FieldValueFn<'_>| {
            if descriptor.log().is_some_and(|log| log.key() == ACTIVE_REQUESTS) && value(&self.engine) == Value::I64(-1) {
                let _ = self.sender.unbounded_send(());
            }
            ControlFlow::Continue(())
        });
    }

    fn flush(&self) -> Result<(), FlushError> {
        Ok(())
    }
}

/// Reads `response`: only the first chunk of its body if `hang_up` is true, else the whole body.
/// Returns the line that the client prints.
async fn read(response: hyper::Result<Response<Incoming>>, hang_up: bool) -> String {
    // When a response body fails, hyper closes the HTTP/1.1 connection or resets the HTTP/2
    // stream, which can happen before the client gets the response head.
    let Ok(response) = response else {
        return "The response failed before the client got it.".to_owned();
    };
    let status = response.status();
    let mut body = response.into_body();
    if hang_up {
        let _ = body.frame().await;
        return format!("The client got {status}, read the first chunk and hung up.");
    }
    match body.collect().await {
        Ok(_) => format!("The client got {status}."),
        Err(_) => format!("The client got {status}, and then the body failed."),
    }
}

#[tokio::main]
#[expect(
    clippy::too_many_lines,
    reason = "the example runs its scenario from top to bottom in one function"
)]
async fn main() {
    let (sender, mut request_ends) = mpsc::unbounded();
    // The sink calls its processors in order, so each event is printed before its signal.
    let sink = Sink::builder("server")
        .processors(vec![
            Arc::new(PrintProcessor(RedactionEngine::builder().suppress_redaction(PUBLIC).build())) as Arc<dyn EventProcessor>,
            Arc::new(RequestEndProcessor {
                engine: RedactionEngine::default(),
                sender,
            }),
        ])
        .build();
    let body_builder = HttpBodyBuilder::new_fake();

    // The application responds to the routes of `BookRoute`, and to the rest with 404. It routes
    // with the same resolver as the middleware.
    let resolver = Arc::new(BookRoute::resolver());
    let routes = Arc::clone(&resolver);
    let bodies = body_builder.clone();
    let handler = Execute::new(move |request: HttpRequest| {
        let chunk = |text: String| Ok::<_, HttpError>(BytesView::copied_from_slice(text.as_bytes(), &bodies));
        let json = Response::builder().header(CONTENT_TYPE, "application/json");
        let response = match routes.resolve(request.method(), request.uri().path()) {
            Ok(BookRoute::GetBook { book }) => json.body(bodies.text(format!(r#"{{"book":"{book}","title":"Dune"}}"#))),
            // One chunk, and then the body waits until the client hangs up.
            Ok(BookRoute::GetChapters { book }) => {
                json.body(bodies.stream(stream::iter([chunk(format!(r#"{{"book":"{book}","chapter":1}}"#))]).chain(stream::pending())))
            }
            // One chunk, and then the body fails.
            Ok(BookRoute::GetPages { book }) => json.body(bodies.stream(stream::iter([
                chunk(format!(r#"{{"book":"{book}","page":1}}"#)),
                Err(HttpError::validation("the page is missing")),
            ]))),
            Ok(BookRoute::Health) => Response::builder().body(bodies.empty()),
            Err(_) => Response::builder().status(StatusCode::NOT_FOUND).body(bodies.empty()),
        }
        .expect("the example builds valid responses");
        async move { Ok::<_, HttpError>(response) }
    });
    let telemetry = Telemetry::server(sink)
        .excluded_paths([BasePath::from_static("/health/")])
        .route_resolver(resolver)
        .route_parameter_data_classes([("book", PUBLIC)])
        .request_header_data_classes([(REQUEST_ID, PUBLIC)]);
    let service = Arc::new((telemetry, handler).into_service());

    // hyper serves each connection, and takes HTTP/1.1 and HTTP/2 on the same port.
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("the example can bind a local port");
    let address = listener.local_addr().expect("a bound listener has an address");
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.expect("the local listener accepts connections");
            let service = Arc::clone(&service);
            let body_builder = body_builder.clone();
            tokio::spawn(async move {
                let hyper_service = hyper::service::service_fn(move |request: Request<Incoming>| {
                    // Frame by frame, so that the trailers of the request body reach the handler too.
                    let request = request.map(|body| {
                        let body = body
                            .map_frame(|frame| frame.map_data(BytesView::from))
                            .map_err(|error| HttpError::from(io::Error::other(error)));
                        body_builder.body(body)
                    });
                    let service = Arc::clone(&service);
                    async move { service.execute(request).await }
                });
                // A connection ends with an error when the client hangs up or a response body
                // fails, as two of the requests do.
                let _ = auto::Builder::new(TokioExecutor::new())
                    .serve_connection(TokioIo::new(stream), hyper_service)
                    .await;
            });
        }
    });

    let requests = async {
        let mut request_id = 0_u32;
        // One connection for each HTTP/1.1 request, because a hang-up or a failed body ends the
        // connection.
        for path in PATHS {
            request_id += 1;
            println!("GET {path} over HTTP/1.1");
            let stream = TcpStream::connect(address).await.expect("the server listens on the address");
            let (mut http1, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
                .await
                .expect("the server takes HTTP/1.1");
            let connection = tokio::spawn(connection);
            // An HTTP/1.1 request has the path in the request line and the host in a header.
            let request = Request::get(path)
                .header(HOST, address.to_string())
                .header(REQUEST_ID, request_id)
                .body(Empty::<BytesView>::new())
                .expect("the example paths are valid URIs");
            let line = read(http1.send_request(request).await, path == HANG_UP_PATH).await;
            connection.abort();
            if path != HEALTH_PATH {
                request_ends.next().await.expect("the sink reports the -1 of each request");
            }
            println!("{line}\n");
        }

        // HTTP/2 hangs up on a request by resetting its stream, so all requests share one
        // connection.
        let stream = TcpStream::connect(address).await.expect("the server listens on the address");
        let (mut http2, connection) = hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(stream))
            .await
            .expect("the server takes HTTP/2");
        tokio::spawn(connection);
        for path in PATHS {
            request_id += 1;
            println!("GET {path} over HTTP/2");
            let request = Request::get(format!("http://{address}{path}"))
                .header(REQUEST_ID, request_id)
                .body(Empty::<BytesView>::new())
                .expect("the example paths are valid URIs");
            let line = read(http2.send_request(request).await, path == HANG_UP_PATH).await;
            if path != HEALTH_PATH {
                request_ends.next().await.expect("the sink reports the -1 of each request");
            }
            println!("{line}\n");
        }
    };
    tokio::time::timeout(TIMEOUT, requests)
        .await
        .expect("the requests finish within the timeout");
}
