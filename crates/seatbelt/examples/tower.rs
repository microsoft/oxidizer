// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example demonstrates how to combine multiple resilience middlewares
//! using the `seatbelt` crate with Tower's `ServiceBuilder` to create a robust
//! execution pipeline compatible with the Tower ecosystem.

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
    // Shared context for resilience middleware
    let context = ResilienceContext::new(Clock::new_tokio()).name("tower_pipeline");

    // Build a Tower service with retry and timeout middlewares using ServiceBuilder.
    // Layers are applied bottom-to-top: timeout wraps the inner service first,
    // then retry wraps the timeout layer.
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
            if fastrand::i16(0..10) > 4 {
                bail!("random failure")
            } else {
                Ok(request)
            }
        });

    // Execute the service using Tower's Service trait
    poll_fn(|cx| service.poll_ready(cx)).await?;
    let output = service.call("value".to_string()).await?;

    println!("execution finished, output: {}", output);

    Ok(())
}
