// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cachet Redis Dashboard — a web UI for inspecting and load-testing Redis-backed caches.
//!
//! ```text
//! cargo run -p cachet_dashboard -- --port 3000 --redis-url redis://127.0.0.1/
//! ```

mod api;
mod load_test;
mod redis_browser;
mod state;

use state::AppState;

fn main() {
    let mut port: u16 = 3000;
    let mut redis_url: Option<String> = None;

    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--port" => {
                i += 1;
                if let Some(val) = args.get(i) {
                    port = val.parse().expect("invalid port number");
                }
            }
            "--redis-url" => {
                i += 1;
                redis_url = args.get(i).cloned();
            }
            other => {
                eprintln!("Unknown argument: {other}");
                std::process::exit(1);
            }
        }
        i += 1;
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime")
        .block_on(run(port, redis_url));
}

async fn run(port: u16, redis_url: Option<String>) {
    let state = AppState::new();

    // Auto-connect if --redis-url was provided.
    if let Some(url) = &redis_url {
        match redis::Client::open(url.as_str()) {
            Ok(client) => match redis::aio::ConnectionManager::new(client).await {
                Ok(conn) => {
                    state.set_connection(conn).await;
                    eprintln!("Connected to Redis at {url}");
                }
                Err(e) => eprintln!("Warning: could not connect to Redis: {e}"),
            },
            Err(e) => eprintln!("Warning: invalid Redis URL: {e}"),
        }
    }

    let app = api::router(state);
    let addr = format!("0.0.0.0:{port}");

    eprintln!("Dashboard listening on http://localhost:{port}");

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| {
            eprintln!("Failed to bind to {addr}: {e}");
            std::process::exit(1);
        });

    axum::serve(listener, app)
        .await
        .unwrap_or_else(|e| eprintln!("Server error: {e}"));
}
