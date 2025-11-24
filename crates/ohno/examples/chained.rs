// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates chaining errors through multiple layers with trace messages.

use ohno::error_trace;

#[ohno::error]
#[from(std::io::Error)]
#[display("Database error (1)")]
struct DatabaseError;

#[ohno::error]
#[from(DatabaseError)]
#[display("Service error (2)")]
struct ServiceError;

#[ohno::error]
#[from(ServiceError)]
#[display("API error (3)")]
struct ApiError;

#[error_trace("connecting to database (1.1)")]
#[error_trace("validating credentials (1.2)")]
#[error_trace("establishing connection pool (1.3)")]
fn database_operation() -> Result<String, DatabaseError> {
    Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied (0)").into())
}

#[error_trace("fetching user data (2.1)")]
#[error_trace("parsing user profile (2.2)")]
#[error_trace("validating user permissions (2.3)")]
fn service_operation() -> Result<String, ServiceError> {
    Ok(database_operation()?)
}

#[error_trace("handling API request (3.1)")]
#[error_trace("processing request payload (3.2)")]
#[error_trace("preparing response (3.3)")]
fn api_operation() -> Result<String, ApiError> {
    Ok(service_operation()?)
}

#[expect(clippy::unwrap_used, reason = "Example code")]
fn main() {
    let error = api_operation().unwrap_err();
    println!("{error}");
    // println!("{error:#?}");
}

/*
Output:

API error (3)
caused by: Service error (2)
caused by: Database error (1)
caused by: access denied (0)
> connecting to database (1.1) (at crates\ohno\examples\chained.rs:25)
> validating credentials (1.2) (at crates\ohno\examples\chained.rs:26)
> establishing connection pool (1.3) (at crates\ohno\examples\chained.rs:27)
> fetching user data (2.1) (at crates\ohno\examples\chained.rs:32)
> parsing user profile (2.2) (at crates\ohno\examples\chained.rs:33)
> validating user permissions (2.3) (at crates\ohno\examples\chained.rs:34)
> handling API request (3.1) (at crates\ohno\examples\chained.rs:39)
> processing request payload (3.2) (at crates\ohno\examples\chained.rs:40)
> preparing response (3.3) (at crates\ohno\examples\chained.rs:41)

With backtrace:

API error (3)
caused by: Service error (2)
caused by: Database error (1)
caused by: access denied (0)
> connecting to database (1.1) (at crates\ohno\examples\chained.rs:25)
> validating credentials (1.2) (at crates\ohno\examples\chained.rs:26)
> establishing connection pool (1.3) (at crates\ohno\examples\chained.rs:27)

Backtrace:
   0: std::backtrace_rs::backtrace::win64::trace
   ...

> fetching user data (2.1) (at crates\ohno\examples\chained.rs:32)
> parsing user profile (2.2) (at crates\ohno\examples\chained.rs:33)
> validating user permissions (2.3) (at crates\ohno\examples\chained.rs:34)

Backtrace:
   0: std::backtrace_rs::backtrace::win64::trace
   ...

> handling API request (3.1) (at crates\ohno\examples\chained.rs:39)
> processing request payload (3.2) (at crates\ohno\examples\chained.rs:40)
> preparing response (3.3) (at crates\ohno\examples\chained.rs:41)

Backtrace:
   0: std::backtrace_rs::backtrace::win64::trace
   ...
*/
