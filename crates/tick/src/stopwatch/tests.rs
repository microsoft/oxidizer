// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::*;

#[test]
fn fast_stopwatch_uses_and_replaces_the_fast_timestamp() {
    let mut stopwatch = Stopwatch::new_fast(SimpleClock::new_system());
    assert_eq!(stopwatch.elapsed(), Duration::ZERO);

    let StopwatchSource::Fast { start } = &mut stopwatch.source else {
        panic!("a system source creates a fast stopwatch");
    };
    *start = crate::fast_stopwatch::Timestamp::for_test(Duration::from_nanos(5));

    assert_eq!(stopwatch.restart(), Duration::ZERO);
    let StopwatchSource::Fast { start } = stopwatch.source else {
        panic!("the test constructs a fast stopwatch");
    };
    assert_eq!(start, crate::fast_stopwatch::Timestamp::for_test(Duration::ZERO));
}
