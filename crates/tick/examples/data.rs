// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example demonstrates how to integrate `SystemTime` with serializable data
//! structures for storage and retrieval.

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

/// A data structure that caches information with timestamp tracking.
///
/// This struct demonstrates how to work with serializable `SystemTime` values
/// using the formatting types provided by the `tick` crate.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct CachedData {
    id: u32,

    // Store the timestamp as Unix seconds.
    last_access: UnixSeconds,
    data: String,
}

impl CachedData {
    const EXPIRATION: Duration = Duration::from_secs(3600);

    /// Creates a new cached data instance with the current timestamp.
    #[must_use]
    pub fn new(id: u32, data: String, clock: &Clock) -> Self {
        Self {
            id,
            last_access: clock.system_time_as::<UnixSeconds>(),
            data,
        }
    }

    /// Returns the timestamp when this data was last accessed.
    #[must_use]
    pub fn last_access(&self) -> SystemTime {
        self.last_access.into()
    }

    /// Updates the data and sets the last access time to the current timestamp.
    pub fn update(&mut self, data: String, clock: &Clock) {
        self.data = data;
        self.last_access = clock.system_time_as::<UnixSeconds>();
    }

    /// Checks if the cached data has expired based on the expiration duration.
    #[must_use]
    pub fn is_expired(&self, clock: &Clock) -> bool {
        let diff = clock
            .system_time()
            .duration_since(self.last_access.into())
            .unwrap_or(Duration::ZERO);

        diff > Self::EXPIRATION
    }
}
