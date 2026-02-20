// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::BTreeMap;
use std::mem;
use std::task::Waker;
use std::time::{Duration, Instant};

/// Unique identifier for a timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TimerKey {
    tick: Instant,

    /// Discriminator that ensures two timer IDs with the same instant can be created.
    discriminator: u32,
}

impl TimerKey {
    const fn new(tick: Instant, id: u32) -> Self {
        Self { tick, discriminator: id }
    }

    /// Determines when the timer will fire.
    pub const fn tick(&self) -> Instant {
        self.tick
    }
}

/// The minimum resolution for timers.
///
/// Timers with a shorter period will be rounded up to this value. The 1ms resolution
/// provides adequate precision for most use cases while minimizing the overhead of
/// timer management. Setting this too low would increase CPU usage from frequent
/// timer checks, while setting it too high would reduce timer precision.
pub(crate) const TIMER_RESOLUTION: Duration = Duration::from_millis(1);

/// Management of one-shot timers, inspired by the [glommio runtime](https://github.com/DataDog/glommio/blob/d3f6e7a2ee7fb071ada163edcf90fc3286424c31/glommio/src/reactor.rs#L80).
///
/// The timers managed by this collection are one-shot, meaning they will not fire again after being triggered.
#[derive(Debug, Default)]
pub(crate) struct Timers {
    /// An ordered map of registered timers.
    ///
    /// Timers are stored in the order in which they will fire.
    /// The [`Waker`] represents the task awaiting the timer.
    wakers: BTreeMap<TimerKey, Waker>,
    last_discriminator: u32,
}

impl Timers {
    pub fn len(&self) -> usize {
        self.wakers.len()
    }

    #[cfg(test)]
    fn contains(&self, id: TimerKey) -> bool {
        self.wakers.contains_key(&id)
    }

    /// Registers a new timer that will fire at the specified instant.
    ///
    /// Returns a unique [`TimerKey`] that can be used to unregister the timer.
    pub fn register(&mut self, when: Instant, waker: Waker) -> TimerKey {
        // We can wrap the discriminator because it's only used to distinguish timers with the same instant,
        // and the actual value can start from 0 again.
        self.last_discriminator = self.last_discriminator.wrapping_add(1);
        let key = TimerKey::new(when, self.last_discriminator);

        self.wakers.insert(key, waker);

        key
    }

    /// Unregisters a timer with the given key.
    ///
    /// If the timer was not found, this operation is a no-op.
    pub fn unregister(&mut self, id: TimerKey) {
        self.wakers.remove(&id);
    }

    /// Returns the instant when the next timer will fire, or `None` if no timers are registered.
    pub fn next_timer(&self) -> Option<Instant> {
        self.wakers.keys().next().map(TimerKey::tick)
    }

    /// Advances timers that are ready to be woken.
    ///
    /// Returns the next timer instant if any timers remain, or `None` if no timers are pending.
    ///
    /// In the future, the signature of this method can be easily expanded to return more
    /// information about the timers that fired and when the next timer fires.
    #[cfg_attr(test, mutants::skip)] // Causes test timeout.
    pub fn advance_timers(&mut self, now: Instant) -> Option<Instant> {
        // We are adding 1ns to the instant to ensure that even timers whose deadline is the current
        // instant are advanced. This is required because of how BTreeMap::split_off works; it does
        // not include keys that are equal to the split key. Adding 1ns to the value makes this work.
        let adjusted_now = now.checked_add(Duration::from_nanos(1)).unwrap_or(now);

        // Check if there are any timers that are ready to be woken.
        match self.wakers.first_entry() {
            Some(entry) => {
                if entry.key().tick() <= adjusted_now {
                    // Split timers into ready and pending timers.
                    let pending = self.wakers.split_off(&TimerKey::new(adjusted_now, 0));
                    let ready = mem::replace(&mut self.wakers, pending);

                    // Invoke the wakers for timers that ticked.
                    for (_, waker) in ready {
                        waker.wake();
                    }

                    // Return the next timer to be fired.
                    return self.next_timer();
                }

                Some(entry.key().tick())
            }
            None => None,
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {

    use super::*;
    use crate::Clock;
    use crate::state::ClockState;

    #[test]
    fn two_timers_same_instant() {
        let mut timers = Timers::default();
        let anchor = Instant::now();
        let when = anchor + Duration::from_secs(2);

        let key1 = timers.register(when, Waker::noop().clone());
        let key2 = timers.register(when, Waker::noop().clone());

        assert_ne!(key1, key2);

        timers.advance_timers(when + Duration::from_secs(1));
        assert_eq!(timers.len(), 0);
    }

    #[test]
    fn advance_timers_ensure_order() {
        let mut timers = Timers::default();
        let anchor = Instant::now();
        let timer_first = anchor + Duration::from_secs(1);
        let timer_second = anchor + Duration::from_secs(2);

        let id1 = timers.register(timer_first, Waker::noop().clone());
        let _id2 = timers.register(timer_second, Waker::noop().clone());

        assert_eq!(timers.len(), 2);
        timers.advance_timers(timer_first + Duration::from_nanos(1));
        assert_eq!(timers.len(), 1);

        assert!(!timers.contains(id1));
        timers.advance_timers(timer_second + Duration::from_nanos(1));
        assert_eq!(timers.len(), 0);
    }

    #[test]
    fn timer_resolution_ensure_correct_value() {
        assert_eq!(TIMER_RESOLUTION, Duration::from_millis(1));
    }

    #[test]
    fn register_timer_with_clock() {
        let clock = Clock::new_system_frozen();
        let id = clock.register_timer(Instant::now(), Waker::noop().clone());

        match clock.clock_state() {
            ClockState::ClockControl(_) => panic!("we are using the system clock"),
            ClockState::System(timers) => assert!(timers.with_timers(|t| t.contains(id))),
        }
    }

    #[test]
    fn unregister_timer_with_clock() {
        let clock = Clock::new_system_frozen();
        let id = clock.register_timer(Instant::now(), Waker::noop().clone());
        clock.unregister_timer(id);
        assert_eq!(clock.clock_state().timers_len(), 0);
    }

    #[test]
    fn unregister_ok() {
        let mut timers = Timers::default();
        let id = timers.register(Instant::now(), Waker::noop().clone());

        assert!(timers.contains(id));
        timers.unregister(id);
        assert!(!timers.contains(id));
    }

    #[test]
    fn next_timer_ok() {
        let mut timers = Timers::default();
        let now = Instant::now();

        let _ = timers.register(now, Waker::noop().clone());
        let _ = timers.register(now.checked_add(Duration::from_secs(1)).unwrap(), Waker::noop().clone());

        assert_eq!(timers.next_timer(), Some(now));
    }

    #[test]
    fn advance_timers_ensure_correct_result() {
        let mut timers = Timers::default();
        let now = Instant::now();
        assert!(timers.advance_timers(now).is_none());

        let next = now.checked_add(Duration::from_secs(1)).unwrap();
        let _ = timers.register(next, Waker::noop().clone());
        assert_eq!(timers.advance_timers(now), Some(next));

        assert_eq!(timers.advance_timers(next), None);
    }
}
