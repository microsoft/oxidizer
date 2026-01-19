// Copyright (c) Microsoft Corporation.

/// Defines the backoff strategy used by resilience middleware for retry operations.
///
/// Backoff strategies control how delays between retry attempts are calculated, providing
/// different approaches to spacing out retries to avoid overwhelming failing systems while
/// balancing responsiveness and resource utilization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Backoff {
    /// Constant backoff strategy that maintains consistent delays between attempts.
    ///
    /// **Example with `2s` base delay:** `2s, 2s, 2s, 2s, ...`
    Constant,

    /// Linear backoff strategy that increases delays proportionally with attempt count.
    ///
    /// **Example with `2s` base delay:** `2s, 4s, 6s, 8s, 10s, ...`
    Linear,

    /// Exponential backoff strategy that doubles delays with each attempt.
    ///
    /// **Example with `2s` base delay:** `2s, 4s, 8s, 16s, 32s, ...`
    Exponential,
}
