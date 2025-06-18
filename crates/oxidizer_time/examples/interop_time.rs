// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// This sample demonstrates the interoperability between the Oxidizer time the "time" crate.
//
// In particular:
//
// - Conversion of Timestamp to time::OffsetDateTime
// - Conversion of Timestamp to zoned time::OffsetDateTime

use oxidizer_time::{Clock, ClockControl};
use time::OffsetDateTime;
use time_tz::timezones::db;
use time_tz::{OffsetDateTimeExt, TimeZone};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let clock_control = ClockControl::new();
    let clock = Clock::with_control(&clock_control);

    let time_display_format =
        time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");

    // Retrieve the current time.
    let now = clock.now();

    // Oxidizer's Timestamp can interop with other crates through the SystemTime.
    // First, we convert the timestamp to system time. Once we have system time,
    // we can convert it to time::OffsetDateTime.
    let timestamp: OffsetDateTime = now.to_system_time().into();
    println!(
        "Current time (UTC): {}",
        timestamp.format(&time_display_format)?
    );

    // Convert the timestamp to date time in Asia/Tokyo. We need to use
    // the time_tz crate for time zone support.
    let zoned = timestamp.to_timezone(db::asia::TOKYO);
    println!(
        "Current time (Asia/Tokyo): {}",
        zoned.format(&time_display_format)?
    );

    // Convert the timestamp to date time in current time zone.
    let system_tz = time_tz::system::get_timezone()?;
    let zoned = timestamp.to_timezone(system_tz);
    println!(
        "Current time ({}): {}",
        system_tz.name(),
        zoned.format(&time_display_format)?
    );

    Ok(())
}