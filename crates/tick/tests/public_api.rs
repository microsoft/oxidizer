// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public API contracts available without optional features.

use std::error::Error as _;
use std::fmt::{self, Write as _};
use std::future::Future as _;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::task::{Context, Poll, Waker};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use tick::runtime::{ClockDriver, InactiveClock, Isolated, Shared};
use tick::{Clock, Delay, Error, SimpleClock, Stopwatch, Timer, TimerTick};

struct FailingWriter;

impl fmt::Write for FailingWriter {
    fn write_str(&mut self, _value: &str) -> fmt::Result {
        Err(fmt::Error)
    }
}

#[test]
fn public_types_have_their_threading_and_clone_contracts() {
    static_assertions::assert_impl_all!(Clock: Send, Sync, Clone, AsRef<Clock>);
    static_assertions::assert_impl_all!(Delay: Send, Sync);
    static_assertions::assert_impl_all!(Error: Send, Sync, thread_aware::ThreadAware);
    static_assertions::assert_impl_all!(InactiveClock: Send, Sync, Clone);
    static_assertions::assert_impl_all!(InactiveClock<Shared>: Send, Sync);
    static_assertions::assert_not_impl_any!(InactiveClock<Shared>: Clone);
    static_assertions::assert_impl_all!(Timer: Send, Sync);
    static_assertions::assert_impl_all!(TimerTick: Send, Sync, UnwindSafe, RefUnwindSafe);
    static_assertions::assert_not_impl_any!(TimerTick: Clone, Copy);
    static_assertions::assert_impl_all!(SimpleClock: Send, Sync, Clone, AsRef<SimpleClock>);
    static_assertions::assert_impl_all!(Stopwatch: Send, Sync);
    static_assertions::assert_impl_all!(ClockDriver: Send, Sync);
    static_assertions::assert_not_impl_all!(ClockDriver: Clone);
    static_assertions::assert_impl_all!(Isolated: Send, Sync, Clone, Copy);
}

#[cfg_attr(miri, ignore)] // Talks to the real OS clock, which Miri cannot do.
#[test]
fn system_clock_reads_current_system_time() {
    let before = SystemTime::now();
    let (clock, _driver) = InactiveClock::new_isolated().activate(Waker::noop().clone());

    assert!(clock.system_time() >= before);
}

#[cfg_attr(miri, ignore)] // Talks to the real OS clock, which Miri cannot do.
#[test]
fn simple_clock_system_time_as_converts_successfully() {
    let before = SystemTime::now();
    let clock = SimpleClock::new_system();
    let converted = clock.system_time_as::<SystemTime>();
    let after = SystemTime::now();

    assert!((before..=after).contains(&converted));
}

#[test]
fn system_clock_instant_uses_the_system_instant_domain() {
    let (clock, _driver) = InactiveClock::new_isolated().activate(Waker::noop().clone());
    let clock_instant = clock.instant();
    let system_instant = Instant::now();

    assert!(system_instant.duration_since(clock_instant) < Duration::from_secs(10));
}

#[cfg(feature = "fast-instant")]
#[test]
fn system_clocks_can_toggle_fast_instant_retrieval() {
    let precise = SimpleClock::new_system();
    let precise_instant = precise.instant();
    let fast = precise.with_fast_instant(true);
    let fast_first = fast.instant();
    let fast_second = fast.instant();
    let precise_again = fast.with_fast_instant(false);

    assert!(precise_instant <= Instant::now());
    assert!(fast_second >= fast_first);
    assert!(precise_again.instant() <= Instant::now());
}

#[cfg_attr(miri, ignore)] // Talks to the real OS clock, which Miri cannot do.
#[test]
fn system_time_as_converts_to_a_supported_public_type() {
    let (clock, _driver) = InactiveClock::new_isolated().activate(Waker::noop().clone());

    let converted: SystemTime = clock.system_time_as();

    assert!(converted <= SystemTime::now());
}

#[cfg_attr(miri, ignore)] // Talks to the real OS clock, which Miri cannot do.
#[test]
fn simple_system_clock_is_monotonic() {
    let clock = SimpleClock::new_system();
    let first = clock.instant();
    let second = clock.instant();

    assert!(second >= first);
}

#[test]
fn stopwatch_accepts_every_public_time_source_shape() {
    let simple_clock = SimpleClock::new_system();
    let (clock, _driver) = InactiveClock::new_isolated().activate(Waker::noop().clone());
    let clock_ref: &Clock = clock.as_ref();

    assert!(std::ptr::eq(clock_ref, &clock));
    let _ = Stopwatch::new(&clock);
    let _ = Stopwatch::new(clock);
    let _ = Stopwatch::new(&simple_clock);
    let _ = Stopwatch::new(simple_clock);
}

#[cfg_attr(miri, ignore)] // Uses real elapsed time.
#[test]
fn system_stopwatch_measures_elapsed_time() {
    let (clock, _driver) = InactiveClock::new_isolated().activate(Waker::noop().clone());
    let watch = clock.stopwatch();

    thread::sleep(Duration::from_millis(1));

    assert!(watch.elapsed() >= Duration::from_millis(1));
}

#[test]
fn zero_delay_is_immediately_ready() {
    let (clock, _driver) = InactiveClock::new_isolated().activate(Waker::noop().clone());
    let mut delay = Delay::new(&clock, Duration::ZERO);

    assert_eq!(poll_delay(&mut delay), Poll::Ready(()));
}

#[test]
fn maximum_delay_stays_pending_without_a_driver_deadline() {
    let (clock, mut driver) = InactiveClock::new_isolated().activate(Waker::noop().clone());
    let mut delay = Delay::new(&clock, Duration::MAX);

    assert_eq!(poll_delay(&mut delay), Poll::Pending);
    assert_eq!(driver.advance_timers(Instant::now()).unwrap(), None);
}

#[test]
fn overflowing_delay_stays_pending_without_a_driver_deadline() {
    let (clock, mut driver) = InactiveClock::new_isolated().activate(Waker::noop().clone());
    let mut delay = Delay::new(&clock, Duration::MAX.saturating_sub(Duration::from_millis(1)));

    assert_eq!(poll_delay(&mut delay), Poll::Pending);
    assert_eq!(driver.advance_timers(Instant::now()).unwrap(), None);
    assert_eq!(poll_delay(&mut delay), Poll::Pending);
}

#[test]
fn dropping_a_delay_removes_its_public_driver_deadline() {
    let (clock, mut driver) = InactiveClock::new_isolated().activate(Waker::noop().clone());
    let mut delay = Delay::new(&clock, Duration::from_mins(1));
    assert_eq!(poll_delay(&mut delay), Poll::Pending);
    assert!(driver.advance_timers(Instant::now()).unwrap().is_some());

    drop(delay);

    assert_eq!(driver.advance_timers(Instant::now()).unwrap(), None);
}

#[cfg_attr(miri, ignore)] // Uses real elapsed time.
#[test]
fn completing_a_delay_removes_its_public_driver_deadline() {
    let (clock, mut driver) = InactiveClock::new_isolated().activate(Waker::noop().clone());
    let period = Duration::from_millis(1);
    let before_registration = Instant::now();
    let mut delay = Delay::new(&clock, period);
    assert_eq!(poll_delay(&mut delay), Poll::Pending);
    let deadline = driver.advance_timers(before_registration).unwrap().unwrap();

    thread::sleep(deadline.saturating_duration_since(Instant::now()));
    assert_eq!(poll_delay(&mut delay), Poll::Ready(()));

    assert_eq!(driver.advance_timers(before_registration).unwrap(), None);
}

#[test]
fn driver_consumed_delay_completes_before_the_precise_clock_reaches_its_deadline() {
    let (clock, mut driver) = InactiveClock::new_isolated().activate(Waker::noop().clone());
    let mut delay = Delay::new(&clock, Duration::from_mins(1));
    let waker = Waker::noop().clone();
    let mut context = Context::from_waker(&waker);
    assert_eq!(std::pin::pin!(&mut delay).poll(&mut context), Poll::Pending);
    let deadline = driver.advance_timers(Instant::now()).unwrap().unwrap();

    assert_eq!(driver.advance_timers(deadline).unwrap(), None);

    assert_eq!(std::pin::pin!(&mut delay).poll(&mut context), Poll::Ready(()));
}

#[test]
fn periodic_timer_is_lazy_and_first_poll_is_pending() {
    let (clock, mut driver) = InactiveClock::new_isolated().activate(Waker::noop().clone());
    let mut timer = Timer::new(&clock, Duration::from_mins(1));
    let now = Instant::now();
    assert_eq!(driver.advance_timers(now).unwrap(), None);

    assert!(poll_timer(&mut timer).is_pending());
    let deadline = driver.advance_timers(now).unwrap().unwrap();
    assert!(poll_timer(&mut timer).is_pending());

    assert_eq!(driver.advance_timers(now).unwrap(), Some(deadline));
}

#[test]
fn registered_periodic_timer_remains_pending_before_its_deadline() {
    let (clock, _driver) = InactiveClock::new_isolated().activate(Waker::noop().clone());
    let mut timer = Timer::new(&clock, Duration::from_mins(1));
    let waker = Waker::noop().clone();
    let mut context = Context::from_waker(&waker);

    assert!(std::pin::pin!(timer.next()).poll(&mut context).is_pending());
    assert!(std::pin::pin!(timer.next()).poll(&mut context).is_pending());
}

#[test]
fn near_maximum_period_never_registers_a_deadline() {
    let (clock, mut driver) = InactiveClock::new_isolated().activate(Waker::noop().clone());
    let mut timer = Timer::new(&clock, Duration::MAX.saturating_sub(Duration::from_millis(1)));

    assert!(poll_timer(&mut timer).is_pending());
    assert!(poll_timer(&mut timer).is_pending());
    assert_eq!(driver.advance_timers(Instant::now()).unwrap(), None);
}

#[test]
fn dropping_a_periodic_timer_removes_its_public_driver_deadline() {
    let (clock, mut driver) = InactiveClock::new_isolated().activate(Waker::noop().clone());
    let mut timer = Timer::new(&clock, Duration::from_mins(1));
    assert!(poll_timer(&mut timer).is_pending());
    assert!(driver.advance_timers(Instant::now()).unwrap().is_some());

    drop(timer);

    assert_eq!(driver.advance_timers(Instant::now()).unwrap(), None);
}

#[cfg_attr(miri, ignore)] // Uses real elapsed time.
#[test]
fn completing_a_periodic_tick_removes_its_public_driver_deadline() {
    let (clock, mut driver) = InactiveClock::new_isolated().activate(Waker::noop().clone());
    let period = Duration::from_millis(1);
    let before_registration = Instant::now();
    let mut timer = Timer::new(&clock, period);
    assert!(poll_timer(&mut timer).is_pending());
    let deadline = driver.advance_timers(before_registration).unwrap().unwrap();

    thread::sleep(deadline.saturating_duration_since(Instant::now()));
    let Poll::Ready(tick) = poll_timer(&mut timer) else {
        panic!("the periodic timer must fire after its deadline");
    };
    assert!(tick.observed_at() >= deadline);

    assert_eq!(driver.advance_timers(before_registration).unwrap(), None);
}

#[test]
fn driver_consumed_periodic_timer_yields_before_precise_time_reaches_its_deadline() {
    let (clock, mut driver) = InactiveClock::new_isolated().activate(Waker::noop().clone());
    let mut timer = Timer::new(&clock, Duration::from_mins(1));
    let waker = Waker::noop().clone();
    let mut context = Context::from_waker(&waker);
    assert!(std::pin::pin!(timer.next()).poll(&mut context).is_pending());
    let deadline = driver.advance_timers(Instant::now()).unwrap().unwrap();

    assert_eq!(driver.advance_timers(deadline).unwrap(), None);

    assert!(std::pin::pin!(timer.next()).poll(&mut context).is_ready());
    assert!(std::pin::pin!(timer.next()).poll(&mut context).is_pending());
}

#[test]
fn clock_gone_error_is_public_and_has_no_source() {
    let (clock, mut driver) = InactiveClock::new_isolated().activate(Waker::noop().clone());
    drop(clock);

    let error = driver.advance_timers(Instant::now()).unwrap_err();

    assert_eq!(error.to_string(), "all clock owners have been dropped");
    assert!(error.source().is_none());
}

#[test]
fn system_time_error_conversion_preserves_message_and_source() {
    let later = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
    let source = SystemTime::UNIX_EPOCH.duration_since(later).unwrap_err();
    let expected = source.to_string();

    let error = Error::from(source);

    assert_eq!(error.to_string(), expected);
    assert!(!error.is_out_of_range());
    assert_eq!(write!(FailingWriter, "{error}"), Err(fmt::Error));
    let source = error.source().unwrap().downcast_ref::<std::time::SystemTimeError>().unwrap();
    assert_eq!(source.to_string(), expected);
}

#[test]
fn pending_delay_keeps_the_driver_alive_after_the_original_clock_is_dropped() {
    let now = Instant::now();
    let (clock, mut driver) = InactiveClock::new_isolated().activate(Waker::noop().clone());
    let mut delay = clock.delay(Duration::from_secs(1));
    assert_eq!(poll_delay(&mut delay), Poll::Pending);
    drop(clock);

    driver.advance_timers(now).unwrap();
    driver.advance_timers(now + Duration::from_secs(2)).unwrap();
    drop(delay);

    driver.advance_timers(now + Duration::from_secs(2)).unwrap_err();
}

fn poll_delay(delay: &mut Delay) -> Poll<()> {
    let mut context = Context::from_waker(Waker::noop());
    std::pin::pin!(delay).poll(&mut context)
}

fn poll_timer(timer: &mut Timer) -> Poll<TimerTick> {
    let mut context = Context::from_waker(Waker::noop());
    std::pin::pin!(timer.next()).poll(&mut context)
}
