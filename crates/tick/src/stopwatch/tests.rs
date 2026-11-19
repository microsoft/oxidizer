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
