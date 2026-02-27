// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{self, Debug};
use std::time::Duration;

use super::args::HedgingDelayArgs;
use super::callbacks::DelayFn;
use super::constants::DEFAULT_HEDGING_DELAY;

/// Defines how hedged requests are scheduled relative to each other.
///
/// `HedgingMode` controls the timing of speculative hedged requests:
///
/// - [`immediate()`][HedgingMode::immediate] launches all hedges at once
/// - [`delay()`][HedgingMode::delay] waits a fixed duration between each hedge
/// - [`dynamic()`][HedgingMode::dynamic] computes the delay per hedge via a callback
///
/// # Examples
///
/// ```rust
/// use std::time::Duration;
/// use seatbelt::hedging::{HedgingMode, HedgingDelayArgs};
///
/// // All requests launch simultaneously
/// let mode = HedgingMode::immediate();
///
/// // Wait 1 second between each hedge launch
/// let mode = HedgingMode::delay(Duration::from_secs(1));
///
/// // Compute delay dynamically
/// let mode = HedgingMode::dynamic(|args: HedgingDelayArgs| {
///     Duration::from_millis(100 * u64::from(args.hedge_index() + 1))
/// });
/// ```
#[derive(Clone)]
pub struct HedgingMode {
    inner: HedgingModeInner,
}

#[derive(Clone)]
enum HedgingModeInner {
    Immediate,
    Delay(Duration),
    Dynamic(DelayFn),
}

impl HedgingMode {
    /// Creates a hedging mode that launches all hedged requests immediately.
    ///
    /// All `N` concurrent requests start at the same time and the first
    /// successful result is returned.
    #[must_use]
    pub fn immediate() -> Self {
        Self {
            inner: HedgingModeInner::Immediate,
        }
    }

    /// Creates a hedging mode that waits a fixed duration before launching each
    /// additional hedged request.
    ///
    /// The original request is always sent immediately. After `delay`, the first
    /// hedge is launched. After another `delay`, the second hedge is launched, etc.
    #[must_use]
    pub fn delay(delay: Duration) -> Self {
        Self {
            inner: HedgingModeInner::Delay(delay),
        }
    }

    /// Creates a hedging mode that computes the delay dynamically for each hedge.
    ///
    /// The `delay_fn` receives [`HedgingDelayArgs`] containing the hedge index
    /// and should return the [`Duration`] to wait before launching that hedge.
    #[must_use]
    pub fn dynamic(delay_fn: impl Fn(HedgingDelayArgs) -> Duration + Send + Sync + 'static) -> Self {
        Self {
            inner: HedgingModeInner::Dynamic(DelayFn::new(delay_fn)),
        }
    }

    pub(crate) fn delay_for(&self, hedge_index: u32) -> Duration {
        match &self.inner {
            HedgingModeInner::Immediate => Duration::ZERO,
            HedgingModeInner::Delay(d) => *d,
            HedgingModeInner::Dynamic(f) => f.call(HedgingDelayArgs { hedge_index }),
        }
    }

    #[cfg(test)]
    pub(crate) fn is_immediate(&self) -> bool {
        matches!(self.inner, HedgingModeInner::Immediate)
    }
}

impl Default for HedgingMode {
    fn default() -> Self {
        Self::delay(DEFAULT_HEDGING_DELAY)
    }
}

impl Debug for HedgingMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.inner {
            HedgingModeInner::Immediate => f.debug_struct("HedgingMode").field("mode", &"immediate").finish(),
            HedgingModeInner::Delay(d) => f.debug_struct("HedgingMode").field("mode", &"delay").field("duration", d).finish(),
            HedgingModeInner::Dynamic(_) => f.debug_struct("HedgingMode").field("mode", &"dynamic").finish(),
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn immediate_is_immediate() {
        let mode = HedgingMode::immediate();
        assert!(mode.is_immediate());
        assert_eq!(mode.delay_for(0), Duration::ZERO);
        assert_eq!(mode.delay_for(5), Duration::ZERO);
    }

    #[test]
    fn delay_returns_fixed_duration() {
        let mode = HedgingMode::delay(Duration::from_secs(1));
        assert!(!mode.is_immediate());
        assert_eq!(mode.delay_for(0), Duration::from_secs(1));
        assert_eq!(mode.delay_for(5), Duration::from_secs(1));
    }

    #[test]
    fn dynamic_computes_per_attempt() {
        let mode = HedgingMode::dynamic(|args| Duration::from_millis(100 * u64::from(args.hedge_index() + 1)));
        assert!(!mode.is_immediate());
        assert_eq!(mode.delay_for(0), Duration::from_millis(100));
        assert_eq!(mode.delay_for(2), Duration::from_millis(300));
    }

    #[test]
    fn default_is_delay_2s() {
        let mode = HedgingMode::default();
        assert!(!mode.is_immediate());
        assert_eq!(mode.delay_for(0), Duration::from_secs(2));
    }

    #[test]
    fn debug_immediate() {
        let s = format!("{:?}", HedgingMode::immediate());
        assert!(s.contains("immediate"));
    }

    #[test]
    fn debug_delay() {
        let s = format!("{:?}", HedgingMode::delay(Duration::from_secs(1)));
        assert!(s.contains("delay"));
    }

    #[test]
    fn debug_dynamic() {
        let s = format!("{:?}", HedgingMode::dynamic(|_| Duration::ZERO));
        assert!(s.contains("dynamic"));
    }

    #[test]
    fn clone_works() {
        let mode = HedgingMode::delay(Duration::from_secs(1));
        #[expect(clippy::redundant_clone, reason = "testing that Clone impl works")]
        let cloned = mode.clone();
        assert_eq!(cloned.delay_for(0), Duration::from_secs(1));

        let mode = HedgingMode::dynamic(|_| Duration::from_millis(500));
        #[expect(clippy::redundant_clone, reason = "testing that Clone impl works")]
        let cloned = mode.clone();
        assert_eq!(cloned.delay_for(0), Duration::from_millis(500));
    }
}
