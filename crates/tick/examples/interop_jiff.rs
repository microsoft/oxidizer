// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example converts `tick` time values to `jiff` values:
//!
//! - Converting `SystemTime` to `jiff::Timestamp`
//! - Converting `SystemTime` to `jiff::Zoned`

use jiff::Timestamp;
use jiff::tz::TimeZone;
use ohno::IntoAppError;
use tick::Clock;

const JIFF_DISPLAY_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

fn main() -> Result<(), ohno::AppError> {
    // Create a frozen test clock.
    let clock = Clock::new_frozen();

    // Retrieve the current time as `jiff::Timestamp`.
    let timestamp = clock.system_time_as::<Timestamp>();
    println!("Current time (UTC): {}", timestamp.strftime(JIFF_DISPLAY_FORMAT));

    // Convert the timestamp to the Asia/Tokyo time zone.
    let zoned = timestamp.in_tz("Asia/Tokyo")?;
    println!("Current time (Asia/Tokyo): {}", zoned.strftime(JIFF_DISPLAY_FORMAT));

    // Convert the timestamp to the local time zone.
    let zoned = timestamp.to_zoned(TimeZone::system());
    println!(
        "Current time ({}): {}",
        TimeZone::system().iana_name().into_app_err("failed to get time zone name")?,
        zoned.strftime(JIFF_DISPLAY_FORMAT)
    );

    // `Zoned` implements `Display` with RFC 9557 output.
    println!("Current time in RFC 9557 format: {zoned}");

    Ok(())
}
