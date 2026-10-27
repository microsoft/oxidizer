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

    /// Distinguishes timer IDs that have the same instant.
    discriminator: u32,
}

impl TimerKey {
    const fn new(tick: Instant, id: u32) -> Self {
        Self { tick, discriminator: id }
    }

    /// Returns the instant when the timer will fire.
    pub(crate) const fn tick(&self) -> Instant {
        self.tick
    }
}

/// The minimum resolution for timers.
///
/// A shorter period is rounded up to this value. One millisecond balances
/// timer precision with the cost of frequent timer checks.
pub(crate) const TIMER_RESOLUTION: Duration = Duration::from_millis(1);

#[cfg(any(feature = "test-util", test))]
pub(crate) type ReadyWakers = BTreeMap<TimerKey, Waker>;

/// Stores one-shot timers.
///
/// This design is based on the
/// [glommio runtime](https://github.com/DataDog/glommio/blob/d3f6e7a2ee7fb071ada163edcf90fc3286424c31/glommio/src/reactor.rs#L80).
/// Each timer fires at most once.
#[derive(Debug, Default)]
pub(crate) struct Timers {
    /// An ordered map of registered timers.
    ///
    /// Timers are stored in firing order. Each [`Waker`] belongs to the task
    /// that waits for the timer.
    wakers: BTreeMap<TimerKey, Waker>,
    last_discriminator: u32,

    // Determines whether the timers are currently being advanced.
    // This value is `false` if timers have not yet been advanced.
    alive: bool,
}

impl Timers {
    pub(crate) fn len(&self) -> usize {
        self.wakers.len()
    }

    pub(crate) fn alive(&self) -> bool {
        self.alive
    }

    #[cfg(test)]
    fn contains(&self, id: TimerKey) -> bool {
        self.wakers.contains_key(&id)
    }

    /// Registers a timer for the specified instant.
    ///
    /// Returns a unique [`TimerKey`] that can be used to unregister the timer.
    pub(crate) fn register(&mut self, when: Instant, waker: Waker) -> TimerKey {
        // We can wrap the discriminator because it's only used to distinguish timers with the same instant,
        // and the actual value can start from 0 again.
        self.last_discriminator = self.last_discriminator.wrapping_add(1);
        let key = TimerKey::new(when, self.last_discriminator);

        self.wakers.insert(key, waker);

        key
    }

    /// Unregisters a timer with the given key.
    ///
    /// Returns the removed waker so the caller can drop it after releasing its timer lock.
    pub(crate) fn unregister(&mut self, id: TimerKey) -> Option<Waker> {
        self.wakers.remove(&id)
    }

    /// Replaces the waker for an existing timer when the awaiting task changes.
    ///
    /// The returned waker must be dropped after releasing the timer lock.
    pub(crate) fn update_waker(&mut self, id: TimerKey, replacement: Waker) -> (bool, Waker) {
        let Some(current) = self.wakers.get_mut(&id) else {
            return (false, replacement);
        };

        if current.will_wake(&replacement) {
            return (true, replacement);
        }

        (true, mem::replace(current, replacement))
    }

    /// Returns the next timer instant, or `None` when no timer is registered.
    pub(crate) fn next_timer(&self) -> Option<Instant> {
        self.wakers.keys().next().map(TimerKey::tick)
    }

    /// Removes ready timers into caller-provided storage.
    ///
    /// Removing existing `BTreeMap` entries does not allocate. Caller-provided storage lets the
    /// caller release its timer lock before waking tasks without allocating a temporary `Vec`.
    pub(crate) fn pop_ready_into(&mut self, now: Instant, ready: &mut [Option<Waker>]) -> (usize, Option<Instant>) {
        self.alive = true;

        let mut count = 0;
        while count < ready.len() {
            let Some(entry) = self.wakers.first_entry() else {
                break;
            };
            if entry.key().tick() > now {
                break;
            }

            let (_, waker) = entry.remove_entry();
            ready[count] = Some(waker);
            count += 1;
        }

        (count, self.next_timer())
    }

    /// Wakes each timer that is ready.
    ///
    /// Returns the next timer instant, or `None` when no timer remains.
    #[cfg(any(feature = "test-util", test))]
    #[cfg_attr(test, mutants::skip)] // Causes test timeout.
    pub(crate) fn advance_timers(&mut self, now: Instant, ready: &mut ReadyWakers) -> Option<Instant> {
        self.alive = true;

        // Check if there are any timers that are ready to be woken.
        match self.wakers.first_entry() {
            Some(entry) => {
                if entry.key().tick() <= now {
                    // Split timers into ready and pending timers.
                    let mut expired = match now.checked_add(Duration::from_nanos(1)) {
                        Some(after_now) => {
                            let pending = self.wakers.split_off(&TimerKey::new(after_now, 0));
                            mem::replace(&mut self.wakers, pending)
                        }
                        None => mem::take(&mut self.wakers),
                    };
                    ready.append(&mut expired);

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
    // `Timers`, `TimerKey`, discriminators, and raw registration are private storage details.
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

        advance_and_wake(&mut timers, when + Duration::from_secs(1));
        assert_eq!(timers.len(), 0);
    }

    #[test]
    fn timer_discriminators_are_sequential() {
        let mut timers = Timers::default();
        let when = Instant::now();

        let first = timers.register(when, Waker::noop().clone());
        let second = timers.register(when, Waker::noop().clone());

        assert_eq!(first.discriminator, 1);
        assert_eq!(second.discriminator, 2);
    }

    #[test]
    fn timer_one_nanosecond_after_now_stays_pending() {
        for last_discriminator in [0, u32::MAX] {
            let mut timers = Timers {
                last_discriminator,
                ..Timers::default()
            };
            let now = Instant::now();
            let next = now + Duration::from_nanos(1);
            let key = timers.register(next, Waker::noop().clone());
            let mut ready = ReadyWakers::new();

            assert_eq!(timers.advance_timers(now, &mut ready), Some(next));
            assert!(timers.contains(key));
            assert!(ready.is_empty());
        }
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
        advance_and_wake(&mut timers, timer_first + Duration::from_nanos(1));
        assert_eq!(timers.len(), 1);

        assert!(!timers.contains(id1));
        advance_and_wake(&mut timers, timer_second + Duration::from_nanos(1));
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
            ClockState::System(storage) => assert!(storage.with_timers(|timers| timers.contains(id))),
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
        _ = timers.unregister(id);
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
    fn pop_ready_into_stops_at_output_capacity() {
        let mut timers = Timers::default();
        let now = Instant::now();
        let _ = timers.register(now, Waker::noop().clone());
        let _ = timers.register(now, Waker::noop().clone());
        let mut ready = [None];

        let (count, next) = timers.pop_ready_into(now, &mut ready);

        assert_eq!(count, ready.len());
        assert!(ready[0].is_some());
        assert_eq!(next, Some(now));
        assert_eq!(timers.len(), 1);
    }

    #[test]
    fn advance_timers_ensure_correct_result() {
        let mut timers = Timers::default();
        let now = Instant::now();
        assert!(advance_and_wake(&mut timers, now).is_none());

        let next = now.checked_add(Duration::from_secs(1)).unwrap();
        let _ = timers.register(next, Waker::noop().clone());
        assert_eq!(advance_and_wake(&mut timers, now), Some(next));

        assert_eq!(advance_and_wake(&mut timers, next), None);
    }

    #[test]
    fn advance_timers_at_maximum_instant() {
        let mut timers = Timers::default();
        let maximum = maximum_instant_after(Instant::now());
        let _ = timers.register(maximum, Waker::noop().clone());

        assert!(advance_and_wake(&mut timers, maximum).is_none());
        assert_eq!(timers.len(), 0);
    }

    fn advance_and_wake(timers: &mut Timers, now: Instant) -> Option<Instant> {
        let mut ready = ReadyWakers::new();
        let next = timers.advance_timers(now, &mut ready);

        for waker in ready.into_values() {
            waker.wake();
        }

        next
    }

    fn maximum_instant_after(anchor: Instant) -> Instant {
        let mut lower = 0;
        let mut upper = Duration::MAX.as_nanos();

        while lower < upper {
            let middle = lower + (upper - lower).div_ceil(2);
            if anchor.checked_add(duration_from_nanos(middle)).is_some() {
                lower = middle;
            } else {
                upper = middle - 1;
            }
        }

        anchor
            .checked_add(duration_from_nanos(lower))
            .expect("binary search only retains durations that can be added to the anchor")
    }

    fn duration_from_nanos(nanos: u128) -> Duration {
        const NANOS_PER_SECOND: u128 = 1_000_000_000;

        Duration::new(
            (nanos / NANOS_PER_SECOND)
                .try_into()
                .expect("Duration::MAX bounds the seconds component to u64"),
            (nanos % NANOS_PER_SECOND)
                .try_into()
                .expect("the nanoseconds remainder is always less than one billion"),
        )
    }
}
