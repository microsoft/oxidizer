// Copyright (c) Microsoft Corporation.

//! This sample demonstrates interoperability between `tick` time and `chrono` time.
//! In particular:
//!
//! - Conversion of Timestamp to `chrono::DateTime`<Utc>
//! - Conversion of Timestamp to `chrono::DateTime`<Local>

use chrono::{DateTime, Local, Utc};
use tick::Clock;
use time_tz::TimeZone;

const CHRONO_DISPLAY_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

fn main() -> anyhow::Result<()> {
    // Create a frozen clock for the current time.
    let clock = Clock::new_frozen();

    // Retrieve the current timestamp.
    let now = clock.timestamp();

    // Tick's Timestamp can interoperate with other crates through SystemTime.
    // First, we convert the timestamp to SystemTime. Once we have SystemTime,
    // we can convert it to chrono::DateTime<Utc>.
    let timestamp: DateTime<Utc> = now.to_system_time().into();

    println!("Current time (UTC): {}", timestamp.format(CHRONO_DISPLAY_FORMAT));

    // Convert the timestamp to date time in Asia/Tokyo. We need to use
    // the chrono_tz crate for timezone support.
    let zoned = timestamp.with_timezone(&chrono_tz::Asia::Tokyo);
    println!("Current time (Asia/Tokyo): {}", zoned.format(CHRONO_DISPLAY_FORMAT));

    // Convert the timestamp to date time in the current timezone.

    let zoned = timestamp.with_timezone(&Local);

    // Retrieving the timezone name is not supported in chrono.
    let tz = time_tz::system::get_timezone()?;
    println!("Current time ({}): {}", tz.name(), zoned.format(CHRONO_DISPLAY_FORMAT));

    Ok(())
}
