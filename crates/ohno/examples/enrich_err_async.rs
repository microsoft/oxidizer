// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(clippy::unwrap_used, reason = "Example code")]
#![expect(missing_docs, reason = "Example code")]

use ohno::enrich_err;

#[ohno::error]
struct AsyncTestError;

// async function with generic parameters
#[enrich_err("generic async function failed with {data}")]
async fn generic_async_function<T: std::fmt::Display + Sync>(data: &T) -> Result<String, AsyncTestError> {
    std::future::ready(()).await;
    Err(AsyncTestError::caused_by(format!("generic error: {data}")))
}

// async function with lifetime parameters
#[enrich_err("async function with lifetime failed")]
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
