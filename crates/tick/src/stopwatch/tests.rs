// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::*;

#[test]
fn coarse_stopwatch_tracks_running_state() {
    let mut stopwatch = Stopwatch::new_coarse(SimpleClock::new_system());
    assert!(!stopwatch.is_running());
    assert_eq!(stopwatch.elapsed(), Duration::ZERO);

    stopwatch.start();
    assert!(stopwatch.is_running());

    let StopwatchSource::Coarse { started_at } = &mut stopwatch.source else {
        panic!("a system source creates a coarse stopwatch");
    };
    *started_at = Some(crate::coarse_time::Timestamp::for_test(Duration::from_nanos(5)));

    stopwatch.restart();
    assert_eq!(stopwatch.elapsed(), Duration::ZERO);

    stopwatch.stop();
    assert!(!stopwatch.is_running());
    let StopwatchSource::Coarse { started_at } = stopwatch.source else {
        panic!("the test constructs a coarse stopwatch");
    };
    assert_eq!(started_at, None);
}

#[test]
fn coarse_start_and_restart_preserve_lifecycle_invariants() {
    let existing = crate::coarse_time::Timestamp::for_test(Duration::from_nanos(5));
    let mut stopwatch = Stopwatch {
        source: StopwatchSource::Coarse {
            started_at: Some(existing),
        },
        elapsed: Duration::from_nanos(7),
    };

    stopwatch.start();
    let StopwatchSource::Coarse { started_at } = &stopwatch.source else {
        panic!("the test constructs a coarse stopwatch");
    };
    assert_eq!(*started_at, Some(existing));

    stopwatch.restart();
    assert_eq!(stopwatch.elapsed, Duration::ZERO);
    let StopwatchSource::Coarse { started_at } = stopwatch.source else {
        panic!("the test constructs a coarse stopwatch");
    };
    assert_eq!(started_at, Some(crate::coarse_time::Timestamp::for_test(Duration::ZERO)));
}
