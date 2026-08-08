// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example demonstrates interoperability between `tick` and the `time` crate.
//!
//! In particular:
//!
//! - Converting `SystemTime` to `time::OffsetDateTime`

use tick::Clock;
use time::OffsetDateTime;

fn main() -> Result<(), ohno::AppError> {
    // Create a frozen clock for the current time.
    let clock = Clock::new_frozen();

    let time_display_format = time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");

    // Retrieve the current time.
    let now = OffsetDateTime::from(clock.system_time());
    println!("Current time (UTC): {}", now.format(&time_display_format)?);

    Ok(())
}
