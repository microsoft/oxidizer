// Copyright (c) Microsoft Corporation.

//! Demonstrates backtrace capture in errors.
//!
//! Run with `RUST_BACKTRACE=1` to see the full backtrace.

use ohno::app::AppError;
use ohno::app_err;

fn level3() -> Result<(), AppError> {
    Err(app_err!("error at deepest level"))
}

fn level2() -> Result<(), AppError> {
    level3()?;
    Ok(())
}

fn level1() -> Result<(), AppError> {
    level2()?;
    Ok(())
}

fn main() {
    let err = level1().unwrap_err();

    println!("Error: {err}\n");

    let backtrace = err.backtrace();
    if backtrace.status() == std::backtrace::BacktraceStatus::Captured {
        println!("Backtrace was successfully captured.");
        println!("Backtrace:\n{backtrace}");
    } else {
        println!("Backtrace was NOT captured. Set RUST_BACKTRACE=1 to see full backtrace details");
    }
}
