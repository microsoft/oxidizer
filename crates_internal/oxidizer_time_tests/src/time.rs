// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures::StreamExt;
    use oxidizer_rt::{BasicThreadState, Runtime, RuntimeOperations};
    use oxidizer_time::{Delay, FutureExt, PeriodicTimer, Stopwatch};

    #[test]
    fn clock_now() {
        Runtime::<BasicThreadState>::new()
            .expect("Could not create runtime")
            .run(async move |context| {
                let timestamp1 = context.clock().now();
                Delay::with_clock(context.clock(), Duration::from_millis(1)).await;
                let timestamp2 = context.clock().now();

                assert!(timestamp1 < timestamp2);
                assert!(
                    timestamp2.checked_duration_since(timestamp1).unwrap()
                        >= Duration::from_millis(1)
                );
            });
    }

    #[test]
    fn stopwatch_elapsed() {
        Runtime::<BasicThreadState>::new()
            .expect("Could not create runtime")
            .run(async move |context| {
                let watch = Stopwatch::with_clock(context.clock());

                Delay::with_clock(context.clock(), Duration::from_millis(1)).await;

                assert!(watch.elapsed() >= Duration::from_millis(1));
            });
    }

    #[test]
    fn timer_tick() {
        Runtime::<BasicThreadState>::new()
            .expect("Could not create runtime")
            .run(async move |context| {
                let watch = Stopwatch::with_clock(context.clock());
                let delay = Delay::with_clock(context.clock(), Duration::from_millis(1));

                delay.await;

                assert!(watch.elapsed() >= Duration::from_millis(1));
            });
    }

    #[test]
    fn periodic_timer_tick() {
        Runtime::<BasicThreadState>::new()
            .expect("Could not create runtime")
            .run(async move |context| {
                let mut timer =
                    PeriodicTimer::with_clock(context.clock(), Duration::from_millis(1));

                let watch = Stopwatch::with_clock(context.clock());
                timer.next().await;
                assert!(watch.elapsed() >= Duration::from_millis(1));

                let watch = Stopwatch::with_clock(context.clock());
                timer.next().await;
                assert!(watch.elapsed() >= Duration::from_millis(1));
            });
    }

    #[test]
    fn timeout_ok() {
        Runtime::<BasicThreadState>::new()
            .expect("Could not create runtime")
            .run(async move |context| {
                let future = Delay::with_clock(context.clock(), Duration::from_secs(10))
                    .timeout_with_clock(Duration::from_millis(1), context.clock());

                future.await.unwrap_err();
            });
    }

    #[test]
    fn timeout_for_fast_operations_is_not_applied() {
        Runtime::<BasicThreadState>::new()
            .expect("Could not create runtime")
            .run(async move |context| {
                let future = RuntimeOperations::yield_now()
                    .timeout_with_clock(Duration::from_millis(1), context.clock());

                future.await.unwrap();
            });
    }
}