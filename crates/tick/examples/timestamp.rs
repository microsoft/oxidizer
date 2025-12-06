// Copyright (c) Microsoft Corporation.

#![expect(
    clippy::use_debug,
    reason = "debug formatting used for example purposes"
)]

//! This file demonstrates the usage of `Timestamp`, which is used to represent
//! an absolute point in time.
//!
//! This includes:
//!
//! - Creating new Timestamp instances
//! - Formatting and parsing Timestamps
//! - Manipulating Timestamp instances

use std::time::{Duration, SystemTime};

use tick::fmt::{Iso8601Timestamp, Rfc2822Timestamp};
use tick::{Clock, Timestamp};

fn main() -> anyhow::Result<()> {
    let clock = Clock::new_frozen_at(Duration::from_secs(3600 * 100));

    creation(&clock)?;
    formatting_and_parsing()?;
    time_manipulation(&clock)?;

    Ok(())
}

fn creation(clock: &Clock) -> anyhow::Result<()> {
    // Retrieve the current absolute UTC time.
    let now = clock.timestamp();

    println!(
        "Current Time (UTC): {}, System Time: {:?}",
        now,
        now.to_system_time()
    );

    // You can also create a timestamp manually using SystemTime.
    let _time = Timestamp::from_system_time(
        SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_secs(10))
            .expect("adding 10 seconds to UNIX epoch never overflows"),
    )?;

    Ok(())
}

fn formatting_and_parsing() -> anyhow::Result<()> {
    // Formatting and parsing - ISO 8601
    let time: Iso8601Timestamp = "2024-07-24T14:30:00Z".parse()?;
    assert_eq!(time.to_string(), "2024-07-24T14:30:00Z");

    // Formatting and parsing - RFC 2822
    let time: Rfc2822Timestamp = "Tue, 15 Nov 1994 12:45:26 -0000".parse()?;
    assert_eq!(time.to_string(), "Tue, 15 Nov 1994 12:45:26 GMT");

    Ok(())
}

fn time_manipulation(clock: &Clock) -> anyhow::Result<()> {
    // Retrieve the current time.
    let earlier = clock.timestamp();
    let now = clock.timestamp();

    // Calculate the difference between two times.
    let diff = clock.timestamp().checked_duration_since(earlier)?;
    println!("Diff: {}ns", diff.as_nanos());

    // Support for duration arithmetic.
    let duration = Duration::from_secs(10);

    let _time = clock.timestamp().checked_add(duration).unwrap();
    let _time = clock.timestamp().checked_sub(duration).unwrap();

    // Comparison operations.
    assert!(earlier <= now);
    assert!(now >= earlier);

    Ok(())
}
