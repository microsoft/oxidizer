// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::*;

#[test]
fn coarse_stopwatch_uses_and_replaces_the_coarse_timestamp() {
    let mut stopwatch = Stopwatch::start_coarse(SimpleClock::new_system());
    assert_eq!(stopwatch.elapsed(), Duration::ZERO);

    let StopwatchSource::Coarse { start } = &mut stopwatch.source else {
        panic!("a system source creates a coarse stopwatch");
    };
    *start = crate::coarse_time::Timestamp::for_test(Duration::from_nanos(5));

    assert_eq!(stopwatch.restart(), Duration::ZERO);
    let StopwatchSource::Coarse { start } = stopwatch.source else {
        panic!("the test constructs a coarse stopwatch");
    };
    assert_eq!(start, crate::coarse_time::Timestamp::for_test(Duration::ZERO));
}
