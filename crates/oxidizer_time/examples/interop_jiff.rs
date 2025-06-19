// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// This sample demonstrates the interoperability between the Oxidizer time the jiff time.
// In particular:
//
// - Conversion of Timestamp to jiff::Timestamp
// - Conversion of Timestamp to jiff::Zoned

use jiff::Timestamp as TimestampJiff;
use jiff::fmt::temporal::DateTimePrinter;
use jiff::tz::TimeZone;
use oxidizer_time::{Clock, ClockControl};

const JIFF_DISPLAY_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let clock_control = ClockControl::new();
    let clock = Clock::with_control(&clock_control);

    // Retrieve the current time.
    let now = clock.now();

    // Oxidizer's Timestamp can interop with other crates through the SystemTime.
    // First, we convert the timestamp to system time. Once we have system time,
    // we can convert it to jiff::Timestamp.
    let timestamp: TimestampJiff = now.to_system_time().try_into()?;
    println!(
        "Current time (UTC): {}",
        timestamp.strftime(JIFF_DISPLAY_FORMAT)
    );

    // Convert the timestamp to date time in Asia/Tokyo.
    let zoned = timestamp.in_tz("Asia/Tokyo")?;
    println!(
        "Current time (Asia/Tokyo): {}",
        zoned.strftime(JIFF_DISPLAY_FORMAT)
    );

    // Convert the timestamp to date time in current time zone.
    let zoned = timestamp.to_zoned(TimeZone::system());
    println!(
        "Current time ({}): {}",
        TimeZone::system()
            .iana_name()
            .ok_or("failed to get time zone name")?,
        zoned.strftime(JIFF_DISPLAY_FORMAT)
    );

    // Temporal is a new pending standard that preserves the time zone information.
    // https://tc39.es/proposal-temporal/docs/index.html
    let mut buff = String::new();
    DateTimePrinter::new().print_zoned(&zoned, &mut buff)?;

    println!("Current time in temporal format: {buff}");

    Ok(())
}