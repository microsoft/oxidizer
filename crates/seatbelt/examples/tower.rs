// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example combines Seatbelt middleware with Tower `ServiceBuilder`.

use std::future::poll_fn;
use std::time::Duration;

use ohno::{app_err, bail};
use seatbelt::retry::Retry;
use seatbelt::timeout::Timeout;
use seatbelt::{RecoveryInfo, ResilienceContext};
use tick::Clock;
use tower::ServiceBuilder;
use tower_service::Service;

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    // Create shared context for the middleware.
    let context = ResilienceContext::new(Clock::new_tokio()).name("tower_pipeline");

    // Build a Tower service with retry and timeout middleware.
    // Tower applies the layers from bottom to top. Timeout wraps the inner
    // service first. Retry then wraps the timeout layer.
    let mut service = ServiceBuilder::new()
        .layer(
            Retry::layer("my_retry", &context)
                .clone_input()
                .recovery_with(|output, _args| match output {
                    Ok(_) => RecoveryInfo::never(),
                    Err(_) => RecoveryInfo::retry(),
                }),
        )
        .layer(
            Timeout::layer("my_timeout", &context)
                .timeout(Duration::from_secs(1))
                .timeout_error(|_args| app_err!("timeout")),
        )
        .service_fn(|request| async move {
            // This threshold prevents a random failure.
            // It keeps the example deterministic in CI.
            if fastrand::i16(0..10) > 10 {
                bail!("random failure")
            }
            Ok(request)
        });

    // Execute the service through the Tower `Service` trait.
    poll_fn(|cx| service.poll_ready(cx)).await?;
    let output = service.call("value".to_string()).await?;

    println!("execution finished, output: {output}");

    Ok(())
}
