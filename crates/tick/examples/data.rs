// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example stores `SystemTime` in a serializable data structure.

use std::time::{Duration, SystemTime};

use tick::Clock;
use tick::fmt::UnixSeconds;

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    // Create a clock for the Tokio runtime.
    let clock = Clock::new_tokio();

    // Deserialize cached data from JSON.
    let json = r#"{
        "id": 1,
        "last_access": 328576,
        "data": "Hello, World!"
    }"#;

    let mut cached_data: CachedData = serde_json::from_str(json)?;

    cached_data.update(String::from("Hello, Rust!"), &clock);
    println!("Last access: {:?}", cached_data.last_access());

    clock.delay(Duration::from_secs(1)).await;

    cached_data.update(String::from("Hello again, Rust!"), &clock);

    println!("Last access: {:?}", cached_data.last_access());

    let json = serde_json::to_string_pretty(&cached_data)?;
    println!("JSON:");
    println!("{json}");

    Ok(())
}

/// Cached data with a last-access timestamp.
///
/// The `UnixSeconds` format stores the timestamp in serialized data.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct CachedData {
    id: u32,

    // Store the timestamp as Unix seconds.
    last_access: UnixSeconds,
    data: String,
}

impl CachedData {
    const EXPIRATION: Duration = Duration::from_hours(1);

    /// Creates cached data with the current clock time.
    #[must_use]
    pub fn new(id: u32, data: String, clock: &Clock) -> Self {
        Self {
            id,
            last_access: clock.system_time_as::<UnixSeconds>(),
            data,
        }
    }

    /// Returns the last-access time.
    #[must_use]
    pub fn last_access(&self) -> SystemTime {
        self.last_access.into()
    }

    /// Updates the data and records the current clock time.
    pub fn update(&mut self, data: String, clock: &Clock) {
        self.data = data;
        self.last_access = clock.system_time_as::<UnixSeconds>();
    }

    /// Returns whether the cached data expired.
    #[must_use]
    pub fn is_expired(&self, clock: &Clock) -> bool {
        let diff = clock
            .system_time()
            .duration_since(self.last_access.into())
            .unwrap_or(Duration::ZERO);

        diff > Self::EXPIRATION
    }
}
