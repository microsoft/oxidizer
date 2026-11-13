// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::future::poll_fn;
use std::task::Poll;

use arty_executor::{Executor, TaskPlacement};
use observed_testing::{TEST_ID, test_emitter};

use super::support::TaskBuilderExt as _;

#[test]
fn lifecycle_events_include_placement_and_poll_count() {
    let (sink, processor) = test_emitter(TEST_ID);
    let mut executor = Executor::builder().sink(sink).build();
    let mut polls = 0;
    let _completion = executor
        .tasks()
        .task(poll_fn(move |cx| {
            polls += 1;
            if polls == 3 {
                Poll::Ready(())
            } else {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }))
        .placement(TaskPlacement::SameThread)
        .register_test();

    for _ in 0..3 {
        _ = executor.execute_cycle();
    }

    let events = processor.events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].name(), "arty.rt.task.spawned");
    assert_eq!(events[0].dimensions()[0], ("placement".to_owned(), "same_thread".into()));
    assert_eq!(events[1].name(), "arty.rt.task.succeeded");
    assert_eq!(events[1].field_metrics()[0].field_key(), "polls");
    assert_eq!(events[1].field_metrics()[0].metric().unit(), "{poll}");
    assert!((events[1].field_metrics()[0].value() - 3.0).abs() < f64::EPSILON);
}

#[test]
fn specific_thread_placement_has_a_distinct_label() {
    let (sink, processor) = test_emitter(TEST_ID);
    let executor = Executor::builder().sink(sink).build();

    let _completion = executor
        .tasks()
        .task(async {})
        .placement(TaskPlacement::SpecificThread)
        .register_test();

    assert_eq!(
        processor.events()[0].dimensions()[0],
        ("placement".to_owned(), "specific_thread".into())
    );
}
