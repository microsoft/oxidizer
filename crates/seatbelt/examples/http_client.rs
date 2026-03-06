// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Resilient HTTP client using `reqwest` with layered timeout and retry middleware.
//!
//! This example builds a `TodoClient` that wraps a [`reqwest::Client`] with three
//! resilience layers, from outermost to innermost:
//!
//! 1. **Outer timeout** - bounds the total wall-clock time, including all retries.
//! 2. **Retry** - retries on network errors and HTTP 5xx server responses.
//! 3. **Inner timeout** - bounds each individual HTTP request attempt.
//!
//! The composed service is type-erased into a [`DynamicService`] and stored inside
//! `TodoClient`, keeping the public API clean regardless of how many layers are
//! stacked.
//!
//! Backend: <https://jsonplaceholder.typicode.com/>

use std::time::Duration;

use layered::{DynamicService, DynamicServiceExt, Execute, Service, Stack};
use seatbelt::retry::{Retry, RetryLayer};
use seatbelt::timeout::{Timeout, TimeoutLayer};
use seatbelt::{Recovery, RecoveryInfo, ResilienceContext};
use tick::Clock;

const BASE_URL: &str = "https://jsonplaceholder.typicode.com";
const OUTER_TIMEOUT: Duration = Duration::from_secs(30);
const INNER_TIMEOUT: Duration = Duration::from_secs(5);

type ServiceInput = reqwest::Request;
type ServiceOutput = Result<reqwest::Response, TodoError>;

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<(), TodoError> {
    let client = TodoClient::default();

    // 1. List the first 5 todos.
    println!("Fetching todos...");
    let todos = client.list_todos(5).await?;
    for todo in &todos {
        println!(
            "  #{} (user {}): {} (completed: {})",
            todo.id, todo.user_id, todo.title, todo.completed
        );
    }

    // 2. Create a new todo.
    println!("\nCreating todo...");
    let new_todo = CreateTodo {
        user_id: 1,
        title: "Write resilient HTTP client".to_string(),
        completed: false,
    };
    let created = client.create_todo(&new_todo).await?;
    println!("  created #{}: {}", created.id, created.title);

    // 3. Get a single todo by ID.
    println!("\nFetching todo #1...");
    let todo = client.get_todo(1).await?;
    println!("  #{}: {} (completed: {})", todo.id, todo.title, todo.completed);

    Ok(())
}

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
struct Todo {
    #[serde(rename = "userId")]
    user_id: u32,
    id: u32,
    title: String,
    completed: bool,
}

#[derive(Debug, serde::Serialize)]
struct CreateTodo {
    #[serde(rename = "userId")]
    user_id: u32,
    title: String,
    completed: bool,
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Domain error for todo operations.
#[ohno::error]
struct TodoError {
    recovery: RecoveryInfo,
}

impl TodoError {
    fn timeout(timeout: Duration) -> Self {
        Self::caused_by(RecoveryInfo::retry(), format!("request timed out after {}s", timeout.as_secs()))
    }
}

impl From<reqwest::Error> for TodoError {
    fn from(error: reqwest::Error) -> Self {
        // do some minimal heuristic to determine if the error is recoverable
        let recovery = if error.is_body() || error.is_timeout() || error.is_connect() {
            RecoveryInfo::retry()
        } else if let Some(status) = error.status() {
            if status.is_server_error() {
                RecoveryInfo::retry()
            } else {
                RecoveryInfo::never()
            }
        } else {
            RecoveryInfo::never()
        };

        Self::caused_by(recovery, error)
    }
}

/// This error holds recovery information for easy integration with resilience pipelines.
impl Recovery for TodoError {
    fn recovery(&self) -> RecoveryInfo {
        self.recovery.clone()
    }
}

// ---------------------------------------------------------------------------
// TodoClient
// ---------------------------------------------------------------------------

/// A simple REST client for the `JSONPlaceholder` `/todos` endpoint.
///
/// Internally, every HTTP request flows through a resilience pipeline
/// (outer timeout -> retry -> inner timeout -> reqwest) that is stored as a
/// type-erased [`DynamicService`].
#[derive(Debug, Clone)]
struct TodoClient {
    client: reqwest::Client,
    service: DynamicService<reqwest::Request, Result<reqwest::Response, TodoError>>,
}

impl Default for TodoClient {
    fn default() -> Self {
        let client = reqwest::Client::new();
        let clock = Clock::new_tokio();
        Self::new(client, clock)
    }
}

impl TodoClient {
    pub fn new(client: reqwest::Client, clock: Clock) -> Self {
        let context = ResilienceContext::new(&clock).name("todo_client");
        let service = (
            outer_timeout(&context),
            retry(&context),
            inner_timeout(&context),
            // Root service - executes the HTTP request via reqwest.
            Execute::new({
                let client = client.clone();
                move |request: reqwest::Request| {
                    let client = client.clone();
                    async move { Ok(client.execute(request).await?) }
                }
            }),
        )
            .into_service()
            .into_dynamic();

        Self { client, service }
    }

    async fn list_todos(&self, limit: u32) -> Result<Vec<Todo>, TodoError> {
        let request = self
            .client
            .get(format!("{BASE_URL}/todos"))
            .query(&[("_limit", limit.to_string())])
            .build()?;

        let response = self.service.execute(request).await?.error_for_status()?.json().await?;

        Ok(response)
    }

    async fn create_todo(&self, todo: &CreateTodo) -> Result<Todo, TodoError> {
        let request = self.client.post(format!("{BASE_URL}/todos")).json(todo).build()?;

        let response = self.service.execute(request).await?.error_for_status()?.json().await?;

        Ok(response)
    }

    async fn get_todo(&self, id: u32) -> Result<Todo, TodoError> {
        let request = self.client.get(format!("{BASE_URL}/todos/{id}")).build()?;

        let response = self.service.execute(request).await?.error_for_status()?.json().await?;

        Ok(response)
    }
}

// ---------------------------------------------------------------------------
// Resilience layer helpers
// ---------------------------------------------------------------------------

/// Outer timeout — caps the total wall-clock time including all retries.
fn outer_timeout(context: &ResilienceContext<ServiceInput, ServiceOutput>) -> TimeoutLayer<ServiceInput, ServiceOutput> {
    Timeout::layer("outer_timeout", context)
        .timeout(OUTER_TIMEOUT)
        .timeout_error(|args| TodoError::timeout(args.timeout()))
}

/// Retry — retries on network errors and 5xx server responses.
fn retry(context: &ResilienceContext<ServiceInput, ServiceOutput>) -> RetryLayer<ServiceInput, ServiceOutput> {
    Retry::layer("retry", context)
        .clone_input_with(|req: &mut reqwest::Request, _args| req.try_clone())
        .recovery_with(|output: &ServiceOutput, _args| match output {
            Ok(resp) if resp.status().is_server_error() => RecoveryInfo::retry(),
            Ok(_) => RecoveryInfo::never(),
            Err(e) => e.recovery(),
        })
}

/// Inner timeout — caps each individual HTTP attempt.
fn inner_timeout(context: &ResilienceContext<ServiceInput, ServiceOutput>) -> TimeoutLayer<ServiceInput, ServiceOutput> {
    Timeout::layer("inner_timeout", context)
        .timeout(INNER_TIMEOUT)
        .timeout_error(|args| TodoError::timeout(args.timeout()))
}
