// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates backtrace configuration options.
//!
//! - `#[backtrace(force)]`: Always capture backtraces
//! - `#[backtrace(disabled)]`: Never capture backtraces
//! - Default: respects `RUST_BACKTRACE` environment variable

#[ohno::error]
#[backtrace(force)]
struct ForcedBacktraceError;

#[ohno::error]
#[backtrace(disabled)]
struct NoBacktraceError;

#[ohno::error]
struct AutoBacktraceError;

fn main() {
    let forced = ForcedBacktraceError::new();
    println!("Forced backtrace error:\n{forced}\n");

    let disabled = NoBacktraceError::new();
    println!("No backtrace error:\n{disabled}\n");

    let auto = AutoBacktraceError::new();
    println!("Auto backtrace error:\n{auto}");
}
