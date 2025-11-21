// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use ohno::error_span;

#[ohno::error]
struct AsyncTestError;

// async function with generic parameters
#[error_span("generic async function failed with {data}")]
async fn generic_async_function<T: std::fmt::Display + Sync>(data: &T) -> Result<String, AsyncTestError> {
    std::future::ready(()).await;
    Err(AsyncTestError::caused_by(format!("generic error: {data}")))
}

// async function with lifetime parameters
#[error_span("async function with lifetime failed")]
async fn async_with_lifetime<'a>(data: &'a str) -> Result<String, AsyncTestError> {
    std::future::ready(()).await;
    Err(AsyncTestError::caused_by(format!("lifetime error: {data}")))
}

#[tokio::main]
async fn main() {
    let result = generic_async_function(&42).await.unwrap_err();
    println!("Generic result: {result}");

    let data = String::from("test");
    let result = async_with_lifetime(&data).await.unwrap_err();
    println!("Lifetime result: {result}");
}
