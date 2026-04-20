// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates using [`ErrorLabel`] and the [`Labeled`] trait for low-cardinality error tagging.

use ohno::{ErrorLabel, Labeled};

#[ohno::error]
struct ApiError {
    label: ErrorLabel,
}

impl ApiError {
    fn io_error(error: std::io::Error) -> Self {
        Self::caused_by("io", error)
    }
}

impl Labeled for ApiError {
    fn label(&self) -> ErrorLabel {
        self.label.clone()
    }
}

fn call_api() -> Result<(), ApiError> {
    Err(ApiError::io_error(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "server took too long",
    )))
}

fn report(error: &ApiError) {
    let label = ErrorLabel::from_error_chain(error, |e| {
        if let Some(e) = e.downcast_ref::<std::io::Error>() {
            return Some(ErrorLabel::from(e.kind()));
        }

        if let Some(e) = e.downcast_ref::<ApiError>() {
            return Some(e.label());
        }

        None
    });

    println!("metric tag: {label}");
}

#[expect(clippy::unwrap_used, reason = "Example code")]
fn main() {
    let err = call_api().unwrap_err();
    report(&err);
}
