// Copyright (c) Microsoft Corporation.

//! This sample demonstrates interoperability between `tick` time and `jiff` time.
//! In particular:
//!
//! - Converting Timestamp to `jiff::Timestamp`
//! - Converting Timestamp to `jiff::Zoned`

use anyhow::Context;
use jiff::Timestamp as TimestampJiff;
use jiff::fmt::temporal::DateTimePrinter;
use jiff::tz::TimeZone;
use tick::Clock;

const JIFF_DISPLAY_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

fn main() -> anyhow::Result<()> {
    // Create a frozen clock for the current time.
    let clock = Clock::new_frozen();

    // Retrieve the current time.
    let now = clock.timestamp();

    // Tick's Timestamp can interoperate with other crates through SystemTime.
    // First, we convert the timestamp to SystemTime. Once we have SystemTime,
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

    // Convert the timestamp to date time in the current time zone.
    let zoned = timestamp.to_zoned(TimeZone::system());
    println!(
        "Current time ({}): {}",
        TimeZone::system()
            .iana_name()
            .context("failed to get time zone name")?,
        zoned.strftime(JIFF_DISPLAY_FORMAT)
    );

    // Temporal is a new pending standard that preserves time zone information.
    // https://tc39.es/proposal-temporal/docs/index.html
    let mut buff = String::new();
    DateTimePrinter::new().print_zoned(&zoned, &mut buff)?;

    println!("Current time in Temporal format: {buff}");

    Ok(())
}
