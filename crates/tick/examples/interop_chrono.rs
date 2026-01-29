// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example demonstrates interoperability between `tick` and the `chrono` crate.
//!
//! In particular:
//!
//! - Converting `SystemTime` to `chrono::DateTime<Utc>`
//! - Converting `SystemTime` to `chrono::DateTime<Local>`

use chrono::{DateTime, Local, Utc};
use ohno::IntoAppError;
use tick::Clock;
use time_tz::TimeZone;

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

    // Retrieving the time zone name is not supported in chrono.
    let tz = time_tz::system::get_timezone().into_app_err("failed to get time zone")?;
    println!("Current time ({}): {}", tz.name(), zoned.format(CHRONO_DISPLAY_FORMAT));

    Ok(())
}
