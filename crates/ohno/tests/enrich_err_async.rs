// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for the `enrich_err` macro with async functions.
#![cfg(not(miri))] // unsupported operation: can't call foreign function `CreateIoCompletionPort` on OS `windows`
#![expect(clippy::drop_non_drop, reason = "this is test code")]

use std::sync::atomic::{AtomicI32, Ordering};

use ohno::{Error, OhnoCore, enrich_err};

#[macro_use]
mod util;

#[derive(Error)]
struct AsyncTestError {
    inner: OhnoCore,
}

#[tokio::test]
async fn simple_async_enrich_err() {
    #[enrich_err("async operation failed")]
    async fn simple_async_failure() -> Result<String, AsyncTestError> {
        // Simulate async work
        std::future::ready(()).await;
        Err(AsyncTestError::caused_by("async error"))
    }

    let error = simple_async_failure().await.unwrap_err();
    let error_display = format!("{error}");
    let lines = error_display.lines().collect::<Vec<_>>();
    assert_eq!(lines.first(), Some(&"async error"));
    assert_trace!(error, "async operation failed");
}

#[tokio::test]
async fn async_enrich_err_with_params() {
    #[enrich_err("async operation failed with {value}")]
    async fn async_with_param(value: i32) -> Result<String, AsyncTestError> {
        // Simulate async work
        std::future::ready(()).await;
        Err(AsyncTestError::caused_by(format!("value: {value}")))
    }

    let error = async_with_param(42).await.unwrap_err();
    let error_display = format!("{error}");
    let lines = error_display.lines().collect::<Vec<_>>();
    assert_eq!(lines.first(), Some(&"value: 42"));
    assert_trace!(error, "async operation failed with 42");
}

// Test that the async function actually returns a Future
#[tokio::test]
async fn async_plus_impl_as_ref() {
    #[enrich_err("async operation failed. Path: {}", path.as_ref().display())]
    async fn simple_async_failure(path: impl AsRef<std::path::Path>) -> Result<String, AsyncTestError> {
        std::future::ready(()).await;
        Err(AsyncTestError::caused_by("async error"))
    }

    // This should compile - confirming the function is actually async
    let future = simple_async_failure("test/path/1.txt");

    // We can use future combinator methods
    let error = future.await.unwrap_err();
    let error_display = format!("{error}");
    let lines = error_display.lines().collect::<Vec<_>>();
    assert_eq!(lines.first(), Some(&"async error"));
    assert_trace!(error, "async operation failed. Path: test/path/1.txt");
}

struct AsyncService {
    counter: i32,
    atomic_counter: AtomicI32,
}

impl AsyncService {
    const fn new() -> Self {
        Self {
            counter: 0,
            atomic_counter: AtomicI32::new(0),
        }
    }
    #[enrich_err("read-only method failed")]
    async fn read_only(&self) -> Result<i32, AsyncTestError> {
        self.atomic_counter.fetch_add(1, Ordering::SeqCst);
        std::future::ready(()).await;
        Err(AsyncTestError::caused_by("counter is zero"))
    }

    #[enrich_err("method with self field access, counter: {}", self.counter)]
    async fn with_self_field(&self) -> Result<i32, AsyncTestError> {
        self.atomic_counter.fetch_add(1, Ordering::SeqCst);
        std::future::ready(()).await;
        Err(AsyncTestError::caused_by("failed with field"))
    }

    #[enrich_err("service method failed with value {value}")]
    async fn with_mut_self_and_copiable_value(&mut self, value: i32) -> Result<i32, AsyncTestError> {
        self.counter += value;
        self.atomic_counter.fetch_add(value, Ordering::SeqCst);
        std::future::ready(()).await;
        Err(AsyncTestError::caused_by("negative value"))
    }

    #[enrich_err("mutable method failed, atomic: {}", self.atomic_counter.load(Ordering::SeqCst))]
    async fn with_mut_self_no_args(&mut self) -> Result<i32, AsyncTestError> {
        self.counter += 1;
        self.atomic_counter.fetch_add(1, Ordering::SeqCst);
        std::future::ready(()).await;
        Err(AsyncTestError::caused_by("mutation failed"))
    }

    #[enrich_err("method failed")] // you can't use message as it consumed in the function
    async fn with_self_and_string(&self, message: String) -> Result<i32, AsyncTestError> {
        std::future::ready(()).await;
        let e = AsyncTestError::caused_by(format!("message was: {message}"));
        drop(message); // ensure message is consumed
        Err(e)
    }

    #[enrich_err("method failed with string ref: {message}")]
    async fn with_self_and_string_ref(&self, message: &String) -> Result<i32, AsyncTestError> {
        std::future::ready(()).await;
        Err(AsyncTestError::caused_by(format!("message was: {message}")))
    }

    #[enrich_err("consuming method failed")]
    async fn consume_self(self) -> Result<i32, AsyncTestError> {
        std::future::ready(()).await;
        let counter = self.counter;
        drop(self); // ensure self is consumed
        Err(AsyncTestError::caused_by(format!("consumed with counter: {counter}")))
    }

    #[enrich_err("consuming method with arg failed, value: {value}")]
    async fn consume_self_with_arg(self, value: i32) -> Result<i32, AsyncTestError> {
        std::future::ready(()).await;
        drop(self); // ensure self is consumed
        Err(AsyncTestError::caused_by(format!("consumed with value: {value}")))
    }

    #[enrich_err("consuming mutable method failed")]
    async fn consume_self_mut(mut self) -> Result<i32, AsyncTestError> {
        self.counter += 1;
        std::future::ready(()).await;
        let counter = self.counter;
        drop(self); // ensure self is consumed
        Err(AsyncTestError::caused_by(format!("consumed mut with counter: {counter}")))
    }
}

#[tokio::test]
async fn async_method_with_mut_self() {
    let mut service = AsyncService::new();
    let error = service.with_mut_self_and_copiable_value(-5).await.unwrap_err();

    let error_display = format!("{error}");
    let lines = error_display.lines().collect::<Vec<_>>();
    assert_eq!(lines.first(), Some(&"negative value"));
    assert_trace!(error, "service method failed with value -5");
}

#[tokio::test]
async fn async_method_with_self() {
    let service = AsyncService::new();
    let error = service.read_only().await.unwrap_err();

    let error_display = format!("{error}");
    let lines = error_display.lines().collect::<Vec<_>>();
    assert_eq!(lines.first(), Some(&"counter is zero"));
    assert_trace!(error, "read-only method failed");
}

#[tokio::test]
async fn async_method_with_self_field_access() {
    let service = AsyncService::new();
    let error = service.with_self_field().await.unwrap_err();

    let error_display = format!("{error}");
    let lines = error_display.lines().collect::<Vec<_>>();
    assert_eq!(lines.first(), Some(&"failed with field"));
    assert_trace!(error, "method with self field access, counter: 0");
}

#[tokio::test]
async fn async_method_with_mut_self_no_args() {
    let mut service = AsyncService::new();
    let error = service.with_mut_self_no_args().await.unwrap_err();

    let error_display = format!("{error}");
    let lines = error_display.lines().collect::<Vec<_>>();
    assert_eq!(lines.first(), Some(&"mutation failed"));
    // The atomic counter is 1 after fetch_add, not 0
    assert_trace!(error, "mutable method failed, atomic: 1");
}

#[tokio::test]
async fn async_method_with_self_and_string() {
    let service = AsyncService::new();
    let message = String::from("test message");
    let error = service.with_self_and_string(message).await.unwrap_err();

    let error_display = format!("{error}");
    let lines = error_display.lines().collect::<Vec<_>>();
    assert_eq!(lines.first(), Some(&"message was: test message"));
    assert_trace!(error, "method failed");
}

#[tokio::test]
async fn async_method_with_self_and_string_ref() {
    let service = AsyncService::new();
    let message = String::from("ref message");
    let error = service.with_self_and_string_ref(&message).await.unwrap_err();

    let error_display = format!("{error}");
    let lines = error_display.lines().collect::<Vec<_>>();
    assert_eq!(lines.first(), Some(&"message was: ref message"));
    assert_trace!(error, "method failed with string ref: ref message");
}

#[tokio::test]
async fn async_method_consume_self() {
    let service = AsyncService::new();
    let error = service.consume_self().await.unwrap_err();

    let error_display = format!("{error}");
    let lines = error_display.lines().collect::<Vec<_>>();
    assert_eq!(lines.first(), Some(&"consumed with counter: 0"));
    assert_trace!(error, "consuming method failed");
}

#[tokio::test]
async fn async_method_consume_self_with_arg() {
    let service = AsyncService::new();
    let error = service.consume_self_with_arg(42).await.unwrap_err();

    let error_display = format!("{error}");
    let lines = error_display.lines().collect::<Vec<_>>();
    assert_eq!(lines.first(), Some(&"consumed with value: 42"));
    assert_trace!(error, "consuming method with arg failed, value: 42");
}

#[tokio::test]
async fn async_method_consume_self_mut() {
    let service = AsyncService::new();
    let error = service.consume_self_mut().await.unwrap_err();

    let error_display = format!("{error}");
    let lines = error_display.lines().collect::<Vec<_>>();
    assert_eq!(lines.first(), Some(&"consumed mut with counter: 1"));
    assert_trace!(error, "consuming mutable method failed");
}

struct CustomFuture;

impl std::future::Future for CustomFuture {
    type Output = Result<i32, AsyncTestError>;

    #[enrich_err("custom future poll failed")]
    fn poll(self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        std::task::Poll::Ready(Err(AsyncTestError::caused_by("poll error")))
    }
}

#[tokio::test]
async fn enrich_err_on_future_poll() {
    let future = CustomFuture;
    let error = future.await.unwrap_err();

    let error_display = format!("{error}");
    let lines = error_display.lines().collect::<Vec<_>>();
    assert_eq!(lines.first(), Some(&"poll error"));
    assert_trace!(error, "custom future poll failed");
}
