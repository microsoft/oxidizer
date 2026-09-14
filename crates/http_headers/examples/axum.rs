// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Axum server using typed request and response headers.

use std::error::Error;

use tokio::net::TcpListener;

#[path = "axum/app.rs"]
mod app;

use app::router;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let app = router();
    let testing = std::env::var_os("IS_TESTING").is_some();
    let address = if testing { "127.0.0.1:0" } else { "127.0.0.1:3000" };
    let listener = TcpListener::bind(address).await?;
    println!("listening on http://{}", listener.local_addr()?);

    let server = axum::serve(listener, app);
    if testing {
        server.with_graceful_shutdown(async {}).await?;
    } else {
        server.await?;
    }

    Ok(())
}
