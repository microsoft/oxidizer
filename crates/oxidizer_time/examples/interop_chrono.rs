// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// This sample demonstrates the interoperability between the Oxidizer time the chrono time.
// In particular:
//
// - Conversion of Timestamp to chrono::DateTime<Utc>
// - Conversion of Timestamp to chrono::DateTime<Local>

use chrono::{DateTime, Local, Utc};
use oxidizer_time::{Clock, ClockControl};
use time_tz::TimeZone;

const CHRONO_DISPLAY_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let clock_control = ClockControl::new();
    let clock = Clock::with_control(&clock_control);

    // Retrieve the current timestamp.
    let now = clock.now();

    // Oxidizer's Timestamp can interop with other crates through the SystemTime.
    // First, we convert the timestamp to system time. Once we have system time,
    // we can convert it to chrono::DateTime<Utc>.
    let timestamp: DateTime<Utc> = now.to_system_time().into();

    println!(
        "Current time (UTC): {}",
        timestamp.format(CHRONO_DISPLAY_FORMAT)
    );

    // Convert the timestamp to date time in Asia/Tokyo. We need to use
    // the chrono_tz crate for time zone support.
    let zoned = timestamp.with_timezone(&chrono_tz::Asia::Tokyo);
    println!(
        "Current time (Asia/Tokyo): {}",
        zoned.format(CHRONO_DISPLAY_FORMAT)
    );

    // Convert the timestamp to date time in current time zone.

    let zoned = timestamp.with_timezone(&Local);

    // Retrieving the time zone name is not supported in chrono.
    let tz = time_tz::system::get_timezone()?;
    println!(
        "Current time ({}): {}",
        tz.name(),
        zoned.format(CHRONO_DISPLAY_FORMAT)
    );

    Ok(())
}