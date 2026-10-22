// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public controlled-clock behavior.

#![cfg(feature = "test-util")]

use std::error::Error as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

#[cfg(feature = "tokio")]
use futures::StreamExt as _;
use futures::{FutureExt as _, Stream};
use thread_aware::{Relocator, ThreadAware, ThreadBuilder};
use tick::runtime::InactiveClock;
use tick::{Clock, ClockControl, Delay, FutureExt as _, PeriodicTimer, SimpleClock};

#[derive(Debug)]
struct WakeCounter(AtomicUsize);

impl Wake for WakeCounter {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn clock_control_is_send_and_sync() {
    static_assertions::assert_impl_all!(ClockControl: Send, Sync);
}

#[test]
fn new_control_starts_at_the_unix_epoch_without_automatic_advance() {
    let control = ClockControl::new();
    let clock = control.to_clock();

    assert_eq!(clock.system_time(), SystemTime::UNIX_EPOCH);
    assert_eq!(clock.system_time(), SystemTime::UNIX_EPOCH);
}

#[test]
fn auto_advance_applies_to_system_time_instant_and_stopwatch() {
    let duration = Duration::from_secs(1);
    let clock = ClockControl::new().auto_advance(duration).to_clock();

    let system_start = clock.system_time();
    assert_eq!(clock.system_time().duration_since(system_start).unwrap(), duration);

    let instant_start = clock.instant();
    assert_eq!(clock.instant().duration_since(instant_start), duration);

    let watch = clock.stopwatch();
    assert_eq!(watch.elapsed(), duration);
}

#[test]
fn advance_moves_public_clock_forward() {
    let control = ClockControl::new();
    let clock = control.to_clock();
    let start = clock.system_time();

    control.advance(Duration::from_secs(1));

    assert_eq!(clock.system_time(), start + Duration::from_secs(1));
}

#[test]
fn advance_millis_moves_public_clock_forward() {
    let control = ClockControl::new();
    let clock = control.to_clock();
    let start = clock.system_time();

    control.advance_millis(123);

    assert_eq!(clock.system_time(), start + Duration::from_millis(123));
}

#[test]
fn set_time_moves_system_and_monotonic_time_backward_together() {
    let control = ClockControl::new();
    let clock = control.to_clock();
    control.set_time(SystemTime::UNIX_EPOCH + Duration::from_secs(10));
    let system_later = clock.system_time();
    let instant_later = clock.instant();

    control.set_time(SystemTime::UNIX_EPOCH + Duration::from_secs(5));

    assert_eq!(system_later.duration_since(clock.system_time()).unwrap(), Duration::from_secs(5));
    assert_eq!(instant_later.duration_since(clock.instant()), Duration::from_secs(5));
}

#[test]
fn new_at_accepts_system_time_and_format_values() {
    let system_time = SystemTime::UNIX_EPOCH + Duration::from_secs(222);
    let direct = ClockControl::new_at(system_time).to_clock();
    assert_eq!(direct.system_time(), system_time);

    #[cfg(feature = "fmt")]
    {
        let timestamp = tick::fmt::UnixSeconds::from_secs(333).unwrap();
        let formatted = ClockControl::new_at(timestamp).to_clock();
        assert_eq!(formatted.system_time(), SystemTime::UNIX_EPOCH + Duration::from_secs(333));
    }
}

#[test]
fn frozen_clock_constructors_preserve_time() {
    let frozen = Clock::new_frozen();
    let frozen_system = frozen.system_time();
    let frozen_instant = frozen.instant();
    assert_eq!(frozen.system_time(), frozen_system);
    assert_eq!(frozen.instant(), frozen_instant);

    let specific = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let at = Clock::new_frozen_at(specific);
    assert_eq!(at.system_time(), specific);
    assert_eq!(at.system_time(), at.system_time());
}

#[test]
fn try_system_time_as_reports_when_the_target_rejects_the_time() {
    #[derive(Debug)]
    struct AlwaysFailsConversion;

    impl TryFrom<SystemTime> for AlwaysFailsConversion {
        type Error = &'static str;

        fn try_from(_: SystemTime) -> Result<Self, Self::Error> {
            Err("conversion always fails")
        }
    }

    let error = Clock::new_frozen().try_system_time_as::<AlwaysFailsConversion>().unwrap_err();

    assert_eq!(error, "conversion always fails");
}

#[test]
fn clock_control_converts_to_clock_by_value_and_reference() {
    let control = ClockControl::new();
    control.advance_millis(12_345);

    let owned = Clock::from(control.clone());
    let borrowed = Clock::from(&control);

    assert_eq!(owned.system_time(), SystemTime::UNIX_EPOCH + Duration::from_millis(12_345));
    assert_eq!(owned.system_time(), borrowed.system_time());
}

#[test]
fn simple_clock_observes_controlled_time() {
    let control = ClockControl::new();
    let clock = control.to_simple_clock();
    let start = clock.system_time();

    control.advance(Duration::from_secs(5));

    assert_eq!(clock.system_time(), start + Duration::from_secs(5));
}

#[cfg(feature = "fast-instant")]
#[test]
fn controlled_simple_clock_ignores_fast_instant_configuration() {
    let control = ClockControl::new();
    let clock = control.to_simple_clock();
    let configured = clock.clone().with_fast_instant(true);

    control.advance(Duration::from_secs(1));

    assert_eq!(configured.instant(), clock.instant());
}

#[test]
fn controlled_simple_clock_stopwatch_observes_controlled_time() {
    let control = ClockControl::new();
    let clock = control.to_simple_clock();
    let watch = clock.stopwatch();

    control.advance(Duration::from_secs(1));

    assert_eq!(watch.elapsed(), Duration::from_secs(1));
}

#[test]
fn frozen_simple_clock_constructors_preserve_time() {
    let frozen = SimpleClock::new_frozen();
    let system_time = frozen.system_time();
    let instant = frozen.instant();
    assert_eq!(frozen.system_time(), system_time);
    assert_eq!(frozen.instant(), instant);

    let specific = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    assert_eq!(SimpleClock::new_frozen_at(specific).system_time(), specific);
}

#[test]
fn simple_clock_conversions_preserve_controlled_time() {
    let control = ClockControl::new();
    let owned_control: SimpleClock = SimpleClock::from(control.clone());
    let borrowed_control: SimpleClock = SimpleClock::from(&control);
    let clock = control.to_clock();
    let borrowed_clock: SimpleClock = SimpleClock::from(&clock);
    let owned_clock: SimpleClock = SimpleClock::from(clock);

    control.advance(Duration::from_secs(3));
    let expected = SystemTime::UNIX_EPOCH + Duration::from_secs(3);

    assert_eq!(
        [
            owned_control.system_time(),
            borrowed_control.system_time(),
            borrowed_clock.system_time(),
            owned_clock.system_time(),
        ],
        [expected; 4]
    );
}

#[test]
fn delay_ignores_real_time_until_controlled_time_advances() {
    let control = ClockControl::new();
    let clock = control.to_clock();
    let mut delay = Delay::new(&clock, Duration::from_millis(1));

    assert_eq!(poll_delay(&mut delay), Poll::Pending);
    control.advance(Duration::from_millis(2));

    assert_eq!(poll_delay(&mut delay), Poll::Ready(()));
}

#[test]
fn delay_fires_at_its_exact_controlled_deadline() {
    let control = ClockControl::new();
    let clock = control.to_clock();
    let mut delay = clock.delay(Duration::from_millis(1));
    assert_eq!(poll_delay(&mut delay), Poll::Pending);

    control.advance(Duration::from_millis(1));

    assert_eq!(poll_delay(&mut delay), Poll::Ready(()));
}

#[test]
fn controlled_advance_wakes_a_ready_delay_once() {
    let control = ClockControl::new();
    let clock = control.to_clock();
    let wakes = Arc::new(WakeCounter(AtomicUsize::new(0)));
    let delay_waker = Waker::from(Arc::clone(&wakes));
    let mut context = Context::from_waker(&delay_waker);
    let mut delay = std::pin::pin!(clock.delay(Duration::from_secs(1)));
    assert_eq!(delay.as_mut().poll(&mut context), Poll::Pending);

    control.advance(Duration::from_millis(500));
    assert_eq!(wakes.0.load(Ordering::Relaxed), 0);
    control.advance(Duration::from_millis(500));
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    control.advance(Duration::from_secs(1));

    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
}

#[test]
fn manual_advance_then_evaluates_enabled_automatic_timer_advance() {
    let control = ClockControl::new();
    let clock = control.to_clock();
    let mut delay = clock.delay(Duration::from_secs(2));
    assert_eq!(poll_delay(&mut delay), Poll::Pending);
    let start = clock.instant();
    let configured = control.auto_advance_timers(true);

    configured.advance(Duration::from_secs(1));

    assert_eq!(clock.instant().duration_since(start), Duration::from_secs(2));
    assert_eq!(poll_delay(&mut delay), Poll::Ready(()));
}

#[test]
fn periodic_timer_ignores_real_time_until_controlled_time_advances() {
    let control = ClockControl::new();
    let clock = control.to_clock();
    let mut timer = PeriodicTimer::new(&clock, Duration::from_millis(1));

    assert_eq!(poll_timer(&mut timer), Poll::Pending);
    control.advance(Duration::from_millis(2));

    assert_eq!(poll_timer(&mut timer), Poll::Ready(Some(())));
}

#[test]
fn periodic_timer_fires_at_its_exact_controlled_deadline() {
    let control = ClockControl::new();
    let clock = control.to_clock();
    let mut timer = PeriodicTimer::new(&clock, Duration::from_millis(1));
    assert_eq!(poll_timer(&mut timer), Poll::Pending);

    control.advance(Duration::from_millis(1));

    assert_eq!(poll_timer(&mut timer), Poll::Ready(Some(())));
}

#[test]
fn zero_period_is_clamped_to_exactly_one_millisecond() {
    let control = ClockControl::new();
    let clock = control.to_clock();
    let mut timer = PeriodicTimer::new(&clock, Duration::ZERO);
    assert_eq!(poll_timer(&mut timer), Poll::Pending);

    control.advance(Duration::from_micros(999));
    assert_eq!(poll_timer(&mut timer), Poll::Pending);

    control.advance(Duration::from_micros(1));
    assert_eq!(poll_timer(&mut timer), Poll::Ready(Some(())));
}

#[test]
fn automatic_timer_advance_completes_delay_and_records_elapsed_time() {
    let clock = ClockControl::new().auto_advance_timers(true).to_clock();
    let watch = clock.stopwatch();

    futures::executor::block_on(clock.delay(Duration::from_secs(30)));

    assert_eq!(watch.elapsed(), Duration::from_secs(30));
}

#[test]
fn automatic_timer_advance_handles_many_sequential_deadlines() {
    let clock = ClockControl::new().auto_advance_timers(true).to_clock();
    let start = clock.instant();

    for millis in 1..=1_000 {
        futures::executor::block_on(clock.delay(Duration::from_millis(millis)));
    }

    assert!(clock.instant().duration_since(start) >= Duration::from_millis(1));
}

#[test]
fn automatic_advance_stops_at_the_configured_limit() {
    let clock = ClockControl::new()
        .auto_advance(Duration::from_millis(550))
        .auto_advance_limit(Duration::from_secs(2))
        .to_clock();
    let anchor = clock.system_time();

    let offsets = std::array::from_fn::<_, 5, _>(|_| clock.system_time().duration_since(anchor).unwrap());

    assert_eq!(
        offsets,
        [
            Duration::from_millis(550),
            Duration::from_millis(1_100),
            Duration::from_millis(1_650),
            Duration::from_secs(2),
            Duration::from_secs(2),
        ]
    );
}

#[test]
fn automatic_timer_advancement_consumes_the_remaining_shared_limit() {
    let control = ClockControl::new();
    let clock = control.to_clock();
    let start = clock.instant();
    let _configured = control
        .auto_advance_timers(true)
        .auto_advance(Duration::from_secs(1))
        .auto_advance_limit(Duration::from_secs(2));
    let mut delay = clock.delay(Duration::from_secs(2));

    assert_eq!(poll_delay(&mut delay), Poll::Pending);
    let after_timer_advance = clock.instant();
    assert_eq!(after_timer_advance.duration_since(start), Duration::from_secs(2));

    assert_eq!(poll_delay(&mut delay), Poll::Ready(()));
    let after_limit = clock.instant();

    assert_eq!(after_limit, after_timer_advance);
}

#[test]
fn timer_beyond_automatic_advance_limit_remains_pending() {
    let clock = ClockControl::new()
        .auto_advance_timers(true)
        .auto_advance(Duration::from_secs(1))
        .auto_advance_limit(Duration::from_secs(1))
        .to_clock();
    let start = clock.instant();
    let mut delay = clock.delay(Duration::from_secs(2));

    assert_eq!(poll_delay(&mut delay), Poll::Pending);
    assert_eq!(clock.instant().duration_since(start), Duration::from_secs(1));
    assert_eq!(poll_delay(&mut delay), Poll::Pending);
}

#[test]
fn timeout_reports_error_with_controlled_time() {
    let clock = ClockControl::new()
        .auto_advance(Duration::from_secs(1))
        .auto_advance_limit(Duration::from_secs(2))
        .to_clock();
    let mut future = clock.delay(Duration::from_secs(10)).timeout(&clock, Duration::from_secs(1));
    let mut context = Context::from_waker(Waker::noop());

    assert!(future.poll_unpin(&mut context).is_pending());

    let Poll::Ready(Err(error)) = future.poll_unpin(&mut context) else {
        panic!("the future must time out on the second poll");
    };
    assert!(error.is_timeout());
    assert_eq!(error.to_string(), "future timed out");
    let source = error.source().unwrap().downcast_ref::<std::io::Error>().unwrap();
    assert_eq!(source.kind(), std::io::ErrorKind::TimedOut);
}

#[test]
fn isolated_relocation_keeps_timer_deadlines_independent() {
    let root = InactiveClock::default();
    let mut inactive_1 = root.clone();
    let mut inactive_2 = root;
    let builder = ThreadBuilder::default();
    let source = builder.clone().with_numa_node(0).build(thread::current().id());
    let destination_id = thread::spawn(|| thread::current().id()).join().unwrap();
    let destination = builder.with_numa_node(1).build(destination_id);
    inactive_1.relocate(Some(&source), &source);
    inactive_2.relocate(Some(&source), &destination);
    let (clock_1, mut driver_1) = inactive_1.activate(Waker::noop().clone());
    let (clock_2, mut driver_2) = inactive_2.activate(Waker::noop().clone());
    let now = Instant::now();
    let mut delay_1 = clock_1.delay(Duration::from_secs(100));
    assert_eq!(poll_delay(&mut delay_1), Poll::Pending);

    assert!(driver_1.advance_timers(now).unwrap().is_some());
    assert_eq!(driver_2.advance_timers(now).unwrap(), None);

    let mut relocated_clock = clock_1;
    relocated_clock.relocate(Some(&source), &destination);
    let mut relocated_delay = relocated_clock.delay(Duration::from_secs(100));
    assert_eq!(poll_delay(&mut relocated_delay), Poll::Pending);

    assert!(driver_2.advance_timers(now).unwrap().is_some());
    assert!(driver_1.advance_timers(now).unwrap().is_some());

    drop((clock_2, delay_1, relocated_delay));
}

#[test]
fn controlled_clock_relocation_keeps_shared_time_and_timers() {
    let control = ClockControl::default();
    let root: InactiveClock = control.clone().into();
    let mut inactive_1 = root.clone();
    let mut inactive_2 = root;
    let (source, _thread_2) = Relocator::between_numa_nodes().relocate(&mut inactive_2);
    let thread_1 = source.as_ref().unwrap();
    inactive_1.relocate(source.as_ref(), thread_1);
    let (clock_1, _driver_1) = inactive_1.activate(Waker::noop().clone());
    let (clock_2, _driver_2) = inactive_2.activate(Waker::noop().clone());
    let first_notifications = Arc::new(WakeCounter(AtomicUsize::new(0)));
    let second_notifications = Arc::new(WakeCounter(AtomicUsize::new(0)));
    let first_waker = Waker::from(Arc::clone(&first_notifications));
    let second_waker = Waker::from(Arc::clone(&second_notifications));
    let mut first_context = Context::from_waker(&first_waker);
    let mut second_context = Context::from_waker(&second_waker);
    let mut first_delay = std::pin::pin!(clock_1.delay(Duration::from_secs(1)));
    let mut second_delay = std::pin::pin!(clock_2.delay(Duration::from_secs(1)));
    assert_eq!(first_delay.as_mut().poll(&mut first_context), Poll::Pending);
    assert_eq!(second_delay.as_mut().poll(&mut second_context), Poll::Pending);
    let start = clock_1.system_time();

    control.advance(Duration::from_secs(1));

    assert_eq!(
        (
            first_notifications.0.load(Ordering::Relaxed),
            second_notifications.0.load(Ordering::Relaxed)
        ),
        (1, 1)
    );
    assert_eq!(
        (clock_1.system_time(), clock_2.system_time()),
        (
            start.checked_add(Duration::from_secs(1)).unwrap(),
            start.checked_add(Duration::from_secs(1)).unwrap()
        )
    );
    assert_eq!(first_delay.as_mut().poll(&mut first_context), Poll::Ready(()));
    assert_eq!(second_delay.as_mut().poll(&mut second_context), Poll::Ready(()));
}

#[test]
fn controlled_driver_finishes_after_the_final_clock_owner_is_dropped() {
    let (clock, mut driver) = InactiveClock::from(ClockControl::new()).activate(Waker::noop().clone());

    assert_eq!(driver.advance_timers(Instant::now()).unwrap(), None);
    drop(clock);

    driver.advance_timers(Instant::now()).unwrap_err();
}

#[test]
fn controlled_driver_has_no_system_parking_deadline() {
    let control = ClockControl::new();
    let (clock, mut driver) = InactiveClock::from(control).activate(Waker::noop().clone());
    let mut delay = clock.delay(Duration::from_secs(1));
    assert_eq!(poll_delay(&mut delay), Poll::Pending);

    assert_eq!(driver.advance_timers(Instant::now()).unwrap(), None);

    drop(delay);
    drop(clock);
    driver.advance_timers(Instant::now()).unwrap_err();
}

#[test]
#[cfg_attr(miri, ignore)]
fn public_debug_output_reports_clock_kind_liveness_and_timer_count() {
    let control = ClockControl::new();
    let controlled = control.to_clock();
    assert_eq!(
        format!("{controlled:#?}"),
        "Clock {\n    kind: \"controlled\",\n    timers: 0,\n    alive: true,\n    ..\n}"
    );

    let mut first = controlled.delay(Duration::from_secs(100));
    let mut second = controlled.delay(Duration::from_secs(200));
    assert_eq!(poll_delay(&mut first), Poll::Pending);
    assert_eq!(poll_delay(&mut second), Poll::Pending);
    assert_eq!(
        format!("{controlled:#?}"),
        "Clock {\n    kind: \"controlled\",\n    timers: 2,\n    alive: true,\n    ..\n}"
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn public_debug_output_reports_clock_control_offset_and_timer_count() {
    let control = ClockControl::new_at(SystemTime::UNIX_EPOCH + Duration::from_secs(123));
    let clock = control.to_clock();
    let mut delay = clock.delay(Duration::from_secs(100));
    assert_eq!(poll_delay(&mut delay), Poll::Pending);
    assert_eq!(
        format!("{control:#?}"),
        "ClockControl {\n    UNIX offset: 123s,\n    timers: 1,\n    ..\n}"
    );

    let negative = ClockControl::new_at(SystemTime::UNIX_EPOCH - Duration::from_secs(123));
    assert_eq!(
        format!("{negative:#?}"),
        "ClockControl {\n    UNIX offset: \"negative\",\n    timers: 0,\n    ..\n}"
    );
}

#[cfg(feature = "tokio")]
#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn timeout_and_periodic_timer_cover_both_tokio_outcomes() {
    let clock = Clock::new_tokio();
    let timeout = async {
        clock.delay(Duration::from_secs(10)).await;
    }
    .timeout(&clock, Duration::from_millis(10))
    .await
    .unwrap_err();
    assert_eq!(timeout.to_string(), "future timed out");

    let value = async {
        clock.delay(Duration::from_millis(1)).await;
        10
    }
    .timeout(&clock, Duration::from_secs(10))
    .await
    .unwrap();
    assert_eq!(value, 10);

    let mut timer = PeriodicTimer::new(&clock, Duration::from_millis(1));
    assert_eq!(timer.next().await, Some(()));
    assert_eq!(timer.next().await, Some(()));
}

fn poll_delay(delay: &mut Delay) -> Poll<()> {
    let mut context = Context::from_waker(Waker::noop());
    std::pin::pin!(delay).poll(&mut context)
}

fn poll_timer(timer: &mut PeriodicTimer) -> Poll<Option<()>> {
    let mut context = Context::from_waker(Waker::noop());
    std::pin::pin!(timer).poll_next(&mut context)
}
