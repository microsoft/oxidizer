// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    clippy::use_debug,
    reason = "debug formatting used for example purposes"
)]

// This file demonstrates the usage of `Timestamp`` time that is used to represent a
// a absolute point in time.
//
// This includes:
//
// - Creating a new Timestamp instance
// - Formatting and parsing Timestamp
// - Manipulating Timestamp instances

use std::error::Error;
use std::time::{Duration, SystemTime};

use oxidizer_time::fmt::{Iso8601Timestamp, Rfc2822Timestamp};
use oxidizer_time::{Clock, ClockControl, Timestamp};

fn main() -> Result<(), Box<dyn Error>> {
    let clock_control = ClockControl::new();
    clock_control.advance(Duration::from_secs(3600 * 100));
    let clock = Clock::with_control(&clock_control);

    creation(&clock)?;
    formatting_and_parsing()?;
    time_manipulation(&clock)?;

    Ok(())
}

fn creation(clock: &Clock) -> Result<(), Box<dyn Error>> {
    // Retrieve the current absolute UTC time.
    let now = clock.now();

    println!(
        "Current Time (UTC): {}, System Time: {:?}",
        now,
        now.to_system_time()
    );

    // You can also create timestamp manually using SystemTime.
    let _time = Timestamp::from_system_time(
        SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_secs(10))
            .expect("adding 10 seconds to UNIX epoch never overflows"),
    )?;

    Ok(())
}

fn formatting_and_parsing() -> Result<(), Box<dyn Error>> {
    // Formatting and parsing - ISO 8601
    let time: Iso8601Timestamp = "2024-07-24T14:30:00Z".parse()?;
    assert_eq!(time.to_string(), "2024-07-24T14:30:00Z");

    // Formatting and parsing - RFC 2822
    let time: Rfc2822Timestamp = "Tue, 15 Nov 1994 12:45:26 -0000".parse()?;
    assert_eq!(time.to_string(), "Tue, 15 Nov 1994 12:45:26 GMT");

    Ok(())
}

fn time_manipulation(clock: &Clock) -> Result<(), Box<dyn Error>> {
    // Retrieve the current time.
    let earlier = clock.now();
    let now = clock.now();

    // Diff between two times.
    let diff = clock.now().checked_duration_since(earlier)?;
    println!("Diff: {}ns", diff.as_nanos());

    // Support for duration.
    let duration = Duration::from_secs(10);

    let _time = clock.now().checked_add(duration).unwrap();
    let _time = clock.now().checked_sub(duration).unwrap();

    // Comparison.
    assert!(earlier <= now);
    assert!(now >= earlier);

    Ok(())
}