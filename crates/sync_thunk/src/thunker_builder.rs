// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

use crate::Thunker;

/// Default maximum number of worker threads.
const DEFAULT_MAX_THREAD_COUNT: usize = 4;

/// Default cool-down interval before an idle worker thread exits.
const DEFAULT_COOL_DOWN_INTERVAL: Duration = Duration::from_secs(10);

/// Default channel capacity (number of pre-allocated work item slots).
const DEFAULT_CHANNEL_CAPACITY: usize = 64;

/// Builder for configuring and constructing a [`Thunker`].
///
/// Obtain an instance via [`Thunker::builder()`], set parameters with chainable methods,
/// and finalize with [`build()`](Self::build). All parameters have sensible defaults:
///
/// | Parameter | Default | Description |
/// |---|---|---|
/// | [`max_thread_count`](Self::max_thread_count) | 4 | Upper bound on worker threads |
/// | [`cool_down_interval`](Self::cool_down_interval) | 10 s | Idle timeout before a worker exits |
/// | [`channel_capacity`](Self::channel_capacity) | 64 | Pre-allocated ring-buffer slots |
///
/// # Examples
///
/// ```
/// use std::time::Duration;
///
/// use sync_thunk::Thunker;
///
/// let thunker = Thunker::builder()
///     .max_thread_count(8)
///     .cool_down_interval(Duration::from_secs(30))
///     .channel_capacity(128)
///     .build();
/// ```
#[derive(Debug, Clone, Copy)]
pub struct ThunkerBuilder {
    pub(crate) max_thread_count: usize,
    pub(crate) cool_down_interval: Duration,
    pub(crate) channel_capacity: usize,
}

impl ThunkerBuilder {
    /// Creates a new builder with default settings.
    pub(crate) fn new() -> Self {
        Self {
            max_thread_count: DEFAULT_MAX_THREAD_COUNT,
            cool_down_interval: DEFAULT_COOL_DOWN_INTERVAL,
            channel_capacity: DEFAULT_CHANNEL_CAPACITY,
        }
    }

    /// Sets the maximum number of worker threads the pool may scale up to.
    ///
    /// Defaults to 4.
    #[must_use]
    pub fn max_thread_count(mut self, count: usize) -> Self {
        self.max_thread_count = count;
        self
    }

    /// Sets the duration a worker thread idles before shutting down.
    ///
    /// Defaults to 10 seconds.
    #[must_use]
    pub fn cool_down_interval(mut self, interval: Duration) -> Self {
        self.cool_down_interval = interval;
        self
    }

    /// Sets the capacity of the pre-allocated work item channel.
    ///
    /// Using a bounded channel avoids per-send heap allocation. If the channel
    /// is full, the async call blocks.
    ///
    /// Defaults to 64.
    #[must_use]
    pub fn channel_capacity(mut self, capacity: usize) -> Self {
        self.channel_capacity = capacity;
        self
    }

    /// Builds and returns a [`Thunker`] with the configured settings.
    ///
    /// Spawns an initial worker thread immediately.
    #[must_use]
    pub fn build(self) -> Thunker {
        Thunker::from_builder(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values() {
        let builder = ThunkerBuilder::new();
        assert_eq!(builder.max_thread_count, DEFAULT_MAX_THREAD_COUNT);
        assert_eq!(builder.cool_down_interval, DEFAULT_COOL_DOWN_INTERVAL);
        assert_eq!(builder.channel_capacity, DEFAULT_CHANNEL_CAPACITY);
    }

    #[test]
    fn max_thread_count_setter() {
        let builder = ThunkerBuilder::new().max_thread_count(16);
        assert_eq!(builder.max_thread_count, 16);
    }

    #[test]
    fn cool_down_interval_setter() {
        let interval = Duration::from_secs(30);
        let builder = ThunkerBuilder::new().cool_down_interval(interval);
        assert_eq!(builder.cool_down_interval, interval);
    }

    #[test]
    fn channel_capacity_setter() {
        let builder = ThunkerBuilder::new().channel_capacity(128);
        assert_eq!(builder.channel_capacity, 128);
    }

    #[test]
    fn chaining_all_setters() {
        let builder = ThunkerBuilder::new()
            .max_thread_count(8)
            .cool_down_interval(Duration::from_millis(500))
            .channel_capacity(32);
        assert_eq!(builder.max_thread_count, 8);
        assert_eq!(builder.cool_down_interval, Duration::from_millis(500));
        assert_eq!(builder.channel_capacity, 32);
    }

    #[test]
    fn debug_impl() {
        let builder = ThunkerBuilder::new();
        let debug = format!("{builder:?}");
        assert!(debug.contains("ThunkerBuilder"));
        assert!(debug.contains("max_thread_count"));
        assert!(debug.contains("cool_down_interval"));
        assert!(debug.contains("channel_capacity"));
    }

    #[test]
    #[expect(clippy::clone_on_copy, reason = "deliberately testing Clone impl")]
    fn clone_and_copy() {
        let builder = ThunkerBuilder::new().max_thread_count(8);
        let cloned = builder.clone();
        let copied = builder; // Copy
        assert_eq!(cloned.max_thread_count, 8);
        assert_eq!(copied.max_thread_count, 8);
    }

    #[test]
    fn build_produces_thunker() {
        let thunker = ThunkerBuilder::new().max_thread_count(2).build();
        assert_eq!(thunker.max_thread_count(), 2);
    }
}
