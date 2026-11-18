// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Controlled timer ownership and timestamp error contracts.

#![cfg(feature = "test-util")]

use std::pin::{Pin, pin};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, SystemTime};

use tick::{ClockControl, Timer};

#[test]
fn gamma_survivor_infinite_timer_polls_do_not_sample_the_auto_advancing_clock() {
    for periodic in [false, true] {
        let clock = ClockControl::new().auto_advance(Duration::from_millis(1)).to_clock();
        let mut delay = clock.delay(Duration::MAX);
        let mut timer = Timer::new(&clock, Duration::MAX);
        let mut cx = Context::from_waker(Waker::noop());
        for _ in 0..3 {
            if periodic {
                let mut next = pin!(timer.next());
                assert!(next.as_mut().poll(&mut cx).is_pending());
            } else {
                assert!(Pin::new(&mut delay).poll(&mut cx).is_pending());
            }
        }
        assert_eq!(clock.system_time(), SystemTime::UNIX_EPOCH);
    }
}

#[test]
fn gamma_survivor_maximum_duration_never_registers_a_waker_and_timeout_keeps_message() {
    use tick::FutureExt;

    let clock = ClockControl::new().to_clock();
    let count = Arc::new(CountWake::default());
    let waker = Waker::from(Arc::clone(&count));
    let mut cx = Context::from_waker(&waker);
    let mut delay = clock.delay(Duration::MAX);
    let mut timer = Timer::new(&clock, Duration::MAX);
    for _ in 0..3 {
        assert!(Pin::new(&mut delay).poll(&mut cx).is_pending());
        let mut next = pin!(timer.next());
        assert!(next.as_mut().poll(&mut cx).is_pending());
        assert_eq!(Arc::strong_count(&count), 2);
    }
    let mut timeout = pin!(std::future::pending::<()>().timeout(&clock, Duration::ZERO));
    let Poll::Ready(Err(error)) = timeout.as_mut().poll(&mut cx) else {
        panic!("a zero timeout of a pending future must return its timeout error");
    };
    assert!(error.is_timeout());
    assert_eq!(error.to_string(), "future timed out");
}

#[derive(Default)]
struct CountWake(AtomicUsize);

impl Wake for CountWake {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn gamma_survivor_cancelled_delays_release_wakers_and_never_fire() {
    let control = ClockControl::new();
    let clock = control.to_clock();
    let count = Arc::new(CountWake::default());
    let waker = Waker::from(Arc::clone(&count));
    let mut cx = Context::from_waker(&waker);
    let mut delays = (1..=70)
        .map(|millis| {
            let mut delay = clock.delay(Duration::from_millis(millis));
            assert_eq!(Pin::new(&mut delay).poll(&mut cx), Poll::Pending);
            delay
        })
        .collect::<Vec<_>>();
    assert_eq!(Arc::strong_count(&count), 72);
    delays.drain(..35);
    assert_eq!(Arc::strong_count(&count), 37);
    control.advance(Duration::from_millis(34));
    assert_eq!(count.0.load(Ordering::Relaxed), 0);
    control.advance(Duration::from_millis(36));
    assert_eq!(count.0.load(Ordering::Relaxed), 35);
    assert_eq!(Arc::strong_count(&count), 2);
    for delay in &mut delays {
        assert_eq!(Pin::new(delay).poll(&mut cx), Poll::Ready(()));
    }
    drop(delays);
    control.advance(Duration::from_secs(1));
    assert_eq!(count.0.load(Ordering::Relaxed), 35);
}

#[test]
fn gamma_survivor_periodic_completion_rearms_and_cancellation_keeps_deadline() {
    let control = ClockControl::new();
    let clock = control.to_clock();
    let count = Arc::new(CountWake::default());
    let waker = Waker::from(Arc::clone(&count));
    let mut cx = Context::from_waker(&waker);
    let mut timer = Timer::new(&clock, Duration::from_millis(5));
    {
        let mut next = pin!(timer.next());
        assert!(next.as_mut().poll(&mut cx).is_pending());
    }
    control.advance(Duration::from_millis(4));
    {
        let mut next = pin!(timer.next());
        assert!(next.as_mut().poll(&mut cx).is_pending());
        control.advance(Duration::from_millis(1));
        let Poll::Ready(tick) = next.as_mut().poll(&mut cx) else {
            panic!("the cancelled next call must preserve its five millisecond deadline");
        };
        assert_eq!(tick.observed_at(), clock.instant());
    }
    assert_eq!(count.0.load(Ordering::Relaxed), 1);
    {
        let mut next = pin!(timer.next());
        assert!(next.as_mut().poll(&mut cx).is_pending());
        control.advance(Duration::from_millis(4));
        assert!(next.as_mut().poll(&mut cx).is_pending());
        control.advance(Duration::from_millis(1));
        assert!(next.as_mut().poll(&mut cx).is_ready());
    }
    assert_eq!(count.0.load(Ordering::Relaxed), 2);
    {
        let mut next = pin!(timer.next());
        assert!(next.as_mut().poll(&mut cx).is_pending());
    }
    drop(timer);
    assert_eq!(Arc::strong_count(&count), 2);
    control.advance(Duration::from_secs(1));
    assert_eq!(count.0.load(Ordering::Relaxed), 2);
}

#[test]
fn gamma_survivor_timer_auto_advance_uses_hint_and_shared_budget() {
    let control = ClockControl::new()
        .auto_advance(Duration::from_millis(2))
        .auto_advance_timers(true)
        .auto_advance_limit(Duration::from_millis(11));
    let clock = control.to_clock();
    let mut cx = Context::from_waker(Waker::noop());
    let mut delay = clock.delay(Duration::from_millis(7));
    assert!(Pin::new(&mut delay).poll(&mut cx).is_pending());
    assert!(Pin::new(&mut delay).poll(&mut cx).is_ready());
    assert_eq!(clock.system_time(), SystemTime::UNIX_EPOCH + Duration::from_millis(9));
    assert_eq!(clock.system_time(), SystemTime::UNIX_EPOCH + Duration::from_millis(11));
    assert_eq!(clock.system_time(), SystemTime::UNIX_EPOCH + Duration::from_millis(11));
    let mut next = clock.delay(Duration::from_millis(1));
    assert!(Pin::new(&mut next).poll(&mut cx).is_pending());
    assert!(Pin::new(&mut next).poll(&mut cx).is_pending());
    control.advance(Duration::from_millis(1));
    assert!(Pin::new(&mut next).poll(&mut cx).is_ready());
}

#[cfg(feature = "fmt")]
#[test]
fn gamma_survivor_unix_seconds_preserves_specific_range_diagnostics() {
    use tick::fmt::UnixSeconds;

    let cases = [
        (
            UnixSeconds::from_secs(u64::MAX).unwrap_err(),
            "the `seconds` is greater than the maximum value that can be represented by `UnixSeconds`",
        ),
        (
            "18446744073709551616".parse::<UnixSeconds>().unwrap_err(),
            "the value is greater than the maximum value that can be represented by `UnixSeconds`",
        ),
        (
            UnixSeconds::try_from(Duration::MAX).unwrap_err(),
            "the `duration` is greater than the maximum value that can be represented by `UnixSeconds`",
        ),
        (
            UnixSeconds::try_from(SystemTime::UNIX_EPOCH - Duration::from_secs(1)).unwrap_err(),
            "the system time is before the Unix epoch and cannot be represented by `UnixSeconds`",
        ),
    ];
    for (error, message) in cases {
        assert!(error.is_out_of_range());
        assert_eq!(error.to_string(), message);
    }
}

#[cfg(all(feature = "fmt", feature = "serde"))]
#[test]
fn gamma_survivor_unix_seconds_invalid_serde_input_returns_error() {
    use tick::fmt::UnixSeconds;

    for input in ["null", "-1", "\"1\"", "1.5", "true", "[]", "{}"] {
        assert!(serde_json::from_str::<UnixSeconds>(input).is_err(), "{input}");
    }
    assert_eq!(serde_json::from_str::<UnixSeconds>("0").unwrap(), UnixSeconds::UNIX_EPOCH);
}
