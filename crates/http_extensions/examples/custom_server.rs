// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A simple HTTP server using `http_extensions` and `hyper` with a custom execution stack.
//!
//! This example demonstrates how to set up a basic HTTP server that listens on `http://localhost:8080`
//! and responds with "Hello, World!" to any incoming request. It also includes middleware using
//! `layered` to log incoming requests and outgoing responses.

use std::sync::Arc;

use bytesbuf::mem::GlobalPool;
use http::Request;
use http_extensions::{HttpBodyBuilder, HttpRequest, HttpResponse, HttpResponseBuilder, RequestHandler};
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server;
use layered::{Execute, Intercept, Stack};
use ohno::ErrorExt;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    // In a real application, the application framework would provide the global memory pool.
    let body_builder = HttpBodyBuilder::new(GlobalPool::new());
    let body_builder_clone = body_builder.clone();

    // Define an execution stack of middlewares
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
                // create a response builder and produce a response
                HttpResponseBuilder::new(&clone).text("Hello, World!").build()
            }
        }),
    );

    serve_with_hyper(stack.into_service(), body_builder_clone).await?;

    Ok(())
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
                let request = request.map(|incoming| body_builder.incoming(incoming));
                let service_cloned = Arc::clone(&service_cloned);

                async move { service_cloned.execute(request).await }
            });

            // Configure the hyper server connection
            let builder = server::conn::auto::Builder::new(TokioExecutor::new());

            let socket = TokioIo::new(socket);
            // Serve the connection with upgrades
            if let Err(e) = builder.serve_connection_with_upgrades(socket, hyper_service).await {
                eprintln!("failed to serve connection: {e}");
            }
        });
    }
}
