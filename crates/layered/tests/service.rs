// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for Service trait implementations.

use futures::executor::block_on;
use layered::Service;

// A simple service that echoes the input.
struct EchoService;

impl Service<String> for EchoService {
    type Out = String;

    async fn execute(&self, input: String) -> Self::Out {
        input
    }
}

#[test]
fn test_echo_service() {
    let service = EchoService;
    let output = block_on(service.execute("Hello, World!".to_string()));
    assert_eq!(output, "Hello, World!");
}

#[test]
fn test_boxed_service() {
    let service: Box<EchoService> = Box::new(EchoService);
    let output = block_on(service.execute("Hello, Boxed World!".to_string()));
    assert_eq!(output, "Hello, Boxed World!");
}

#[test]
fn test_arc_service() {
    let service: std::sync::Arc<EchoService> = std::sync::Arc::new(EchoService);
    let output = block_on(service.execute("Hello, Arc World!".to_string()));
    assert_eq!(output, "Hello, Arc World!");
}
