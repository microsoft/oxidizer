// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// This example demonstrates how to use the time module in the oxidizer crate to work with serializable data.
// Especially, storing and retrieving the timestamps.

use std::error::Error;
use std::time::Duration;

use oxidizer_time::fmt::UnixSecondsTimestamp;
use oxidizer_time::{Clock, Delay, Timestamp};

async fn data_example(clock: Clock) -> Result<(), Box<dyn Error>> {
    // Deserialize cached data from JSON
    let json = r#"{
        "id": 1,
        "last_access": 328576,
        "data": "Hello, World!"
    }"#;

    let mut cached_data: CachedData = serde_json::from_str(json)?;

    cached_data.update(String::from("Hello, Rust!"), &clock);
    println!("Last access: {}", cached_data.last_access());

    Delay::with_clock(&clock, Duration::from_secs(1)).await;

    cached_data.update(String::from("Hello again, Rust!"), &clock);

    println!("Last access: {}", cached_data.last_access());

    let json = serde_json::to_string_pretty(&cached_data)?;
    println!("JSON:");
    println!("{json}");

    Ok(())
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct CachedData {
    id: u32,

    // Store the timestamp as an ISO 8601 string
    last_access: UnixSecondsTimestamp,
    data: String,
}

impl CachedData {
    const EXPIRATION: Duration = Duration::from_secs(3600);

    #[must_use]
    pub fn new(id: u32, data: String, clock: &Clock) -> Self {
        Self {
            id,
            last_access: clock.now().into(),
            data,
        }
    }

    #[must_use]
    pub fn last_access(&self) -> Timestamp {
        self.last_access.into()
    }

    pub fn update(&mut self, data: String, clock: &Clock) {
        self.data = data;
        self.last_access = clock.now().into();
    }

    #[must_use]
    pub fn is_expired(&self, clock: &Clock) -> bool {
        let diff = clock
            .now()
            .checked_duration_since(self.last_access)
            .unwrap_or(Duration::ZERO);

        diff > Self::EXPIRATION
    }
}

#[path = "utils/mini_runtime.rs"]
mod runtime;

fn main() {
    runtime::MiniRuntime::execute(data_example).unwrap();
}