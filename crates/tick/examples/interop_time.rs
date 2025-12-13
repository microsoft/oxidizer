// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example demonstrates interoperability between `tick` and the `time` crate.
//!
//! In particular:
//!
//! - Converting `SystemTime` to `time::OffsetDateTime`
//! - Converting `SystemTime` to a zoned `time::OffsetDateTime`

use tick::Clock;
use time::OffsetDateTime;
use time_tz::timezones::db;
use time_tz::{OffsetDateTimeExt, TimeZone};

fn main() -> anyhow::Result<()> {
    // Create a frozen clock for the current time.
    let clock = Clock::new_frozen();

    let time_display_format = time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");

    // Retrieve the current time.
    let now = clock.system_time_as::<OffsetDateTime>();
    println!("Current time (UTC): {}", now.format(&time_display_format)?);

    // Convert the timestamp to date-time in Asia/Tokyo. We need to use
    // the time_tz crate for time zone support.
    let zoned = now.to_timezone(db::asia::TOKYO);
    println!("Current time (Asia/Tokyo): {}", zoned.format(&time_display_format)?);

    // Convert the timestamp to date-time in the current time zone.
    let system_tz = time_tz::system::get_timezone()?;
    let zoned = now.to_timezone(system_tz);
    println!("Current time ({}): {}", system_tz.name(), zoned.format(&time_display_format)?);

    Ok(())
}
