// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example converts `tick` time values to `chrono` values:
//!
//! - Converting `SystemTime` to `chrono::DateTime<Utc>`
//! - Converting `SystemTime` to `chrono::DateTime<Local>`

use chrono::{DateTime, Local, Utc};
use tick::Clock;

const CHRONO_DISPLAY_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

fn main() {
    // Create a frozen test clock.
    let clock = Clock::new_frozen();

    // Retrieve the current timestamp.
    let timestamp = DateTime::<Utc>::from(clock.system_time());
    println!("Current time (UTC): {}", timestamp.format(CHRONO_DISPLAY_FORMAT));

    // Use `chrono_tz` to convert the timestamp to the Asia/Tokyo time zone.
    let zoned = timestamp.with_timezone(&chrono_tz::Asia::Tokyo);
    println!("Current time (Asia/Tokyo): {}", zoned.format(CHRONO_DISPLAY_FORMAT));

    // Convert the timestamp to the local time zone.
    let zoned = timestamp.with_timezone(&Local);
    println!("Current time (local): {}", zoned.format(CHRONO_DISPLAY_FORMAT));
}
