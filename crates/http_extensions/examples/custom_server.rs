// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example defines an HTTP server with `http_extensions` and `hyper`.
//!
//! The server listens on `http://localhost:8080` and returns "Hello, World!" for each request.
//! Middleware from `layered` logs requests and responses.

use std::sync::Arc;
use std::time::Duration;

use bytesbuf::BytesView;
use bytesbuf::mem::GlobalPool;
use futures::TryStreamExt;
use http::Request;
use http_body_util::BodyExt;
use http_extensions::{HttpBodyBuilder, HttpBodyOptions, HttpRequest, HttpResponse, HttpResponseBuilder, RequestHandler};
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server;
use layered::{Execute, Intercept, Stack};
use ohno::ErrorExt;
use tick::{Clock, FutureExt};
use tokio::net::TcpListener;

const SERVER_LIFETIME: Duration = Duration::from_secs(2);

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    let clock = Clock::new_tokio();

    // An application framework normally provides the global memory pool.
    let body_builder = HttpBodyBuilder::new(GlobalPool::new(), &clock);
    let body_builder_clone = body_builder.clone();

    // Define the middleware stack.
    let stack = (
        Intercept::layer()
            .on_input(|req: &HttpRequest| println!("received request, uri: {}", req.uri()))
            .on_output(|result: &http_extensions::Result<HttpResponse>| match result {
                Ok(response) => println!("response produced, status: {}", response.status()),
                Err(error) => println!("response error, message: {}", error.message()),
            }),
        Execute::new(move |_req: HttpRequest| {
            let clone = body_builder.clone();
            async move {
                // Build the response.
                HttpResponseBuilder::new(&clone).text("Hello, World!").build()
            }
        }),
    );

    let Ok(res) = serve_with_hyper(stack.into_service(), body_builder_clone)
        .timeout(&clock, SERVER_LIFETIME)
        .await
    else {
        println!("automatically shutting down the server after {} seconds", SERVER_LIFETIME.as_secs());
        return Ok(());
    };

    res
}

/// Maps a hyper [`Incoming`] body to an [`HttpBody`][http_extensions::HttpBody].
///
/// [`HttpBodyBuilder::stream`] wraps the byte stream.
/// This crate does not need the `hyper` feature for this operation.
fn map_incoming_to_http_body(incoming: Incoming, body_builder: &HttpBodyBuilder) -> http_extensions::HttpBody {
    let stream = incoming
        .into_data_stream()
        .map_ok(BytesView::from)
        .map_err(|e| http_extensions::HttpError::other(e, recoverable::RecoveryInfo::unknown(), "hyper"));

    body_builder.stream(stream, &HttpBodyOptions::default())
}

async fn serve_with_hyper<T: RequestHandler + 'static>(service: T, body_builder: HttpBodyBuilder) -> Result<(), ohno::AppError> {
    let service = Arc::new(service);
    let listener = TcpListener::bind("127.0.0.1:8080").await?;

    println!("Listening on: {}", listener.local_addr()?);

    loop {
        let (socket, _remote_addr) = listener.accept().await?;
        let service_cloned = Arc::clone(&service);
        let body_builder = body_builder.clone();

        tokio::spawn(async move {
            let hyper_service = hyper::service::service_fn(move |request: Request<Incoming>| {
                let request = request.map(|incoming| map_incoming_to_http_body(incoming, &body_builder));
                let service_cloned = Arc::clone(&service_cloned);

                async move { service_cloned.execute(request).await }
            });

            // Set the hyper server connection configuration.
            let builder = server::conn::auto::Builder::new(TokioExecutor::new());

            let socket = TokioIo::new(socket);
            // Serve the connection and support protocol upgrades.
            if let Err(e) = builder.serve_connection_with_upgrades(socket, hyper_service).await {
                eprintln!("failed to serve connection: {e}");
            }
        });
    }
}
