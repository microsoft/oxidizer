// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::marker::PhantomData;

use thread_aware::ThreadAware;
use thread_aware::affinity::{MemoryAffinity, PinnedAffinity};

use crate::Clock;
use crate::runtime::clock_driver::ClockDriver;
use crate::state::ClockState;

/// Marker for an [`InactiveClock`] backed by per-core isolated timer storage.
///
/// This is the default mode. Clones can be relocated to different threads via
/// [`ThreadAware::relocated`], producing independent timer storage per core. Suitable for
/// thread-per-core runtimes.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct Isolated;

/// Marker for an [`InactiveClock`] backed by a single shared timer set.
///
/// Created via [`InactiveClock::new_shared`]. The resulting `InactiveClock<Shared>`
/// intentionally does **not** implement [`Clone`] or [`ThreadAware`]: there is exactly one
/// shared timer set advanced by exactly one driver, so cloning or relocation would create
/// configurations the driver could not advance correctly.
#[derive(Debug)]
#[non_exhaustive]
pub struct Shared;

/// An inactive clock that must be activated before time operations can be performed.
///
/// This type represents a clock in an inactive state that cannot perform any time-related
/// operations until activated. It is parameterized by a mode marker (`S`):
///
/// - [`Isolated`] (default): per-core timer storage. The inactive clock can be cloned and
///   relocated across threads, with each thread getting an independent timer set on
///   activation.
/// - [`Shared`]: a single shared timer set advanced by a single driver. Use
///   [`InactiveClock::new_shared`] to construct one. Does not implement [`Clone`] or
///   [`ThreadAware`].
///
/// To begin using the clock, call [`InactiveClock::activate`] to get a working [`Clock`] instance and
/// its associated [`ClockDriver`]. The driver is responsible for advancing timers and must
/// be called periodically by the runtime.
///
/// # Examples
///
/// ```rust
/// use tick::runtime::InactiveClock;
///
/// let inactive = InactiveClock::default();
/// let (clock, driver) = inactive.activate();
///
/// // Use the clock for time operations
/// let now = clock.instant();
///
/// // Driver must be advanced periodically (typically by the runtime)
/// // driver.advance_timers(std::time::Instant::now());
/// ```
///
/// # Thread-per-core runtimes
///
/// In thread-per-core architectures, clone the `InactiveClock` and
/// [`relocate`](thread_aware::ThreadAware::relocated) each clone to its target thread before
/// activation. Relocation creates per-core timer storage, so each thread gets an independent set
/// of timers with no cross-thread lock contention.
#[derive(Debug)]
pub struct InactiveClock<S = Isolated> {
    state: ClockState,
    _marker: PhantomData<fn() -> S>,
}

impl Clone for InactiveClock<Isolated> {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            _marker: PhantomData,
        }
    }
}

impl Default for InactiveClock<Isolated> {
    fn default() -> Self {
        Self {
            state: ClockState::new_system(),
            _marker: PhantomData,
        }
    }
}

impl ThreadAware for InactiveClock<Isolated> {
    fn relocated(self, source: MemoryAffinity, destination: PinnedAffinity) -> Self {
        Self {
            state: self.state.relocated(source, destination),
            _marker: PhantomData,
        }
    }
}

#[cfg(any(feature = "rt-shared", test))]
impl InactiveClock<Shared> {
    /// Creates an [`InactiveClock`] backed by a single shared timer set.
    ///
    /// The returned value is intentionally not [`Clone`] and does not implement
    /// [`ThreadAware`]: a `Shared` clock has exactly one timer set that must be advanced by
    /// exactly one [`ClockDriver`]. This is the construction mode used by
    /// [`Clock::new_tokio`][crate::Clock::new_tokio].
    #[must_use]
    pub fn new_shared() -> Self {
        Self {
            state: ClockState::new_system_shared(),
            _marker: PhantomData,
        }
    }
}

impl<S> InactiveClock<S> {
    /// Activates the clock for time operations.
    ///
    /// Consumes this inactive clock and returns a working [`Clock`] instance along with
    /// its [`ClockDriver`]. The driver must be called periodically to advance timers.
    ///
    /// # Returns
    ///
    /// A tuple containing:
    /// - [`Clock`] - The activated clock instance for time operations
    /// - [`ClockDriver`] - Driver that advances timers (must be polled by caller)
    #[must_use]
    pub fn activate(self) -> (Clock, ClockDriver) {
        let state = self.state;
        let clock = Clock::new(state.clone());
        let driver = ClockDriver::new(state);

        (clock, driver)
    }
}

#[cfg(any(feature = "test-util", test))]
impl From<crate::ClockControl> for InactiveClock<Isolated> {
    fn from(control: crate::ClockControl) -> Self {
        Self {
            state: ClockState::ClockControl(control),
            _marker: PhantomData,
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;
    use crate::ClockControl;

    #[test]
    fn assert_types() {
        assert_impl_all!(InactiveClock: Send, Sync, Clone);
        assert_impl_all!(InactiveClock<Shared>: Send, Sync);
        assert_not_impl_any!(InactiveClock<Shared>: Clone);
    }

    #[test]
    fn activate_ok() {
        let inactive_clock = InactiveClock::default();
        let (clock, driver) = inactive_clock.activate();
        assert!(matches!(clock.clock_state(), ClockState::System(_)));
        assert!(matches!(&driver.state, &ClockState::System(_)));
    }

    #[test]
    fn activate_shared_ok() {
        let inactive_clock = InactiveClock::new_shared();
        let (clock, driver) = inactive_clock.activate();
        assert!(matches!(clock.clock_state(), ClockState::System(_)));
        assert!(matches!(&driver.state, &ClockState::System(_)));
    }

    #[test]
    fn activate_with_fake_clock_ok() {
        let inactive_clock = InactiveClock::from(ClockControl::new());
        let (clock, driver) = inactive_clock.activate();
        assert!(matches!(clock.clock_state(), ClockState::ClockControl(_)));
        assert!(matches!(&driver.state, &ClockState::ClockControl(_)));
    }
}
