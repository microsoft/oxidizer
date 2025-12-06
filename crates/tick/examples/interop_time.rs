// Copyright (c) Microsoft Corporation.

//! This sample demonstrates interoperability between `tick` and the "time" crate.
//!
//! In particular:
//!
//! - Converting Timestamp to `time::OffsetDateTime`
//! - Converting Timestamp to zoned `time::OffsetDateTime`

use tick::Clock;
use time::OffsetDateTime;
use time_tz::timezones::db;
use time_tz::{OffsetDateTimeExt, TimeZone};

fn main() -> anyhow::Result<()> {
    // Create a frozen clock for the current time.
    let clock = Clock::new_frozen();

    let time_display_format =
        time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");

    // Retrieve the current time.
    let now = clock.timestamp();

    // Tick's timestamp can interop with other crates through SystemTime.
    // First, we convert the timestamp to system time. Once we have system time,
    // we can convert it to time::OffsetDateTime.
    let timestamp: OffsetDateTime = now.to_system_time().into();
    println!(
        "Current time (UTC): {}",
        timestamp.format(&time_display_format)?
    );

    // Convert the timestamp to date time in Asia/Tokyo. We need to use
    // the time_tz crate for timezone support.
    let zoned = timestamp.to_timezone(db::asia::TOKYO);
    println!(
        "Current time (Asia/Tokyo): {}",
        zoned.format(&time_display_format)?
    );

    // Convert the timestamp to date time in the current timezone.
    let system_tz = time_tz::system::get_timezone()?;
    let zoned = timestamp.to_timezone(system_tz);
    println!(
        "Current time ({}): {}",
        system_tz.name(),
        zoned.format(&time_display_format)?
    );

    Ok(())
}
