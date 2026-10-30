// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Axum server using typed request and response headers.
//!
//! The server uses an OS-assigned local port by default. Use `--port PORT` to select a port.
//! Use `--terminate-after SECONDS` to stop it automatically. Normal runs continue until stopped.
//! Anvil uses two seconds when this option is not present.

use std::error::Error;
use std::time::Duration;

use argh::FromArgs;
use tokio::net::TcpListener;

#[path = "axum/app.rs"]
mod app;

use app::router;

const ANVIL_TERMINATE_AFTER: Duration = Duration::from_secs(2);

/// Command-line options for the server example.
#[derive(FromArgs)]
struct ServerOptions {
    /// local TCP port; the operating system selects a port when this is zero
    #[argh(option, default = "0")]
    port: u16,
    /// stop the server after this number of seconds
    #[argh(option)]
    terminate_after: Option<u64>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let options: ServerOptions = argh::from_env();
    let app = router();
    let listener = TcpListener::bind(("127.0.0.1", options.port)).await?;
    println!("listening on http://{}", listener.local_addr()?);

    let terminate_after = options
        .terminate_after
        .map(Duration::from_secs)
        .or_else(|| std::env::var_os("IS_TESTING").is_some().then_some(Duration::ZERO))
        .or_else(|| std::env::var_os("ANVIL_EXAMPLE").is_some().then_some(ANVIL_TERMINATE_AFTER));

    let server = axum::serve(listener, app);
    if let Some(terminate_after) = terminate_after {
        server.with_graceful_shutdown(tokio::time::sleep(terminate_after)).await?;
    } else {
        server.await?;
    }

    Ok(())
}
