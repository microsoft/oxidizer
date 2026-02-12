// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example demonstrates interoperability between `tick` and the `chrono` crate.
//!
//! In particular:
//!
//! - Converting `SystemTime` to `chrono::DateTime<Utc>`
//! - Converting `SystemTime` to `chrono::DateTime<Local>`

use chrono::{DateTime, Local, Utc};
use tick::Clock;

const CHRONO_DISPLAY_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

fn main() -> Result<(), ohno::AppError> {
    // Create a frozen clock for the current time.
    let clock = Clock::new_frozen();

    // Retrieve the current timestamp.
    let timestamp = clock.system_time_as::<DateTime<Utc>>();
    println!("Current time (UTC): {}", timestamp.format(CHRONO_DISPLAY_FORMAT));

    // Convert the timestamp to date time in Asia/Tokyo. We need to use
    // the chrono_tz crate for time zone support.
    let zoned = timestamp.with_timezone(&chrono_tz::Asia::Tokyo);
    println!("Current time (Asia/Tokyo): {}", zoned.format(CHRONO_DISPLAY_FORMAT));

    // Convert the timestamp to date time in the current time zone.
    let zoned = timestamp.with_timezone(&Local);
    println!("Current time (local): {}", zoned.format(CHRONO_DISPLAY_FORMAT));

    Ok(())
}
