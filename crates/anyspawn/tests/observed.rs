// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Enrichment propagation through the public runtime constructors.

#![cfg(all(feature = "observed", any(feature = "arty", feature = "tokio")))]

use std::future::poll_fn;
use std::task::Poll;

use anyspawn::{JoinError, Spawner};
use observed::enrichment::EnrichFnExt;
use observed::{Enrichment, Severity, Sink, emit};
use observed_testing::events::ProbeEvent;
use observed_testing::{ExpectedEvent, MockProcessor, TEST_ID, test_emitter};

#[derive(Clone, Debug, Enrichment)]
struct Request {
    #[unredacted]
    request_id: i64,
}

fn expected(value: i64, request_id: Option<i64>) -> ExpectedEvent {
    let event = ExpectedEvent::new("test.probe", Severity::Info).dimension("value", value.to_string());
    match request_id {
        Some(id) => event.dimension("request_id", id),
        None => event,
    }
}

fn assert_events(processor: &MockProcessor, expected: &[ExpectedEvent]) {
    let events = processor.events();
    assert_eq!(events.len(), expected.len(), "{events:?}");
    for event in expected {
        assert!(events.iter().any(|actual| actual == event), "missing {event:?} in {events:?}");
    }
}

fn exercise_spawns(spawner: &Spawner, plain: &Spawner, sink: &Sink, processor: &MockProcessor) -> Result<(), JoinError> {
    assert!(format!("{spawner:?}").contains(&format!("{sink:?}")));
    let mut expectations = Vec::new();
    for request_id in [10, 20] {
        let clone = spawner.clone();
        let (task, anywhere, blocking) = (|| {
            let task_sink = sink.clone();
            let nested_spawner = clone.clone();
            let task = clone.spawn(async move {
                let mut first_poll = true;
                poll_fn(|cx| {
                    if first_poll {
                        emit!(&task_sink, ProbeEvent::new(1));
                        first_poll = false;
                        cx.waker().wake_by_ref();
                        Poll::Pending
                    } else {
                        emit!(&task_sink, ProbeEvent::new(2));
                        Poll::Ready(())
                    }
                })
                .await;
                nested_spawner
                    .spawn(async move {
                        emit!(&task_sink, ProbeEvent::new(6));
                        42
                    })
                    .await
            });
            let anywhere = clone.spawn_anywhere(sink.clone(), |sink| {
                emit!(&sink, ProbeEvent::new(3));
                async move {
                    emit!(&sink, ProbeEvent::new(4));
                    43
                }
            });
            let blocking_sink = sink.clone();
            let blocking = clone.spawn_blocking(move || {
                emit!(&blocking_sink, ProbeEvent::new(5));
                44
            });
            (task, anywhere, blocking)
        })
        .enrich(sink, Request { request_id })();

        assert_eq!(futures::executor::block_on(task)??, 42);
        assert_eq!(futures::executor::block_on(anywhere)?, 43);
        assert_eq!(futures::executor::block_on(blocking)?, 44);
        expectations.extend((1..=6).map(|value| expected(value, Some(request_id))));
        assert_events(processor, &expectations);
    }

    // Neither the caller nor later tasks inherit an earlier spawn's context.
    emit!(sink, ProbeEvent::new(7));
    let task_sink = sink.clone();
    futures::executor::block_on(spawner.spawn(async move {
        emit!(&task_sink, ProbeEvent::new(8));
    }))?;
    futures::executor::block_on(spawner.spawn_anywhere(sink.clone(), |sink| async move {
        emit!(&sink, ProbeEvent::new(9));
    }))?;
    let blocking_sink = sink.clone();
    futures::executor::block_on(spawner.spawn_blocking(move || {
        emit!(&blocking_sink, ProbeEvent::new(10));
    }))?;
    expectations.extend((7..=10).map(|value| expected(value, None)));

    // Enabling the feature does not opt spawners without a sink into transfer.
    let (task, anywhere, blocking) = (|| {
        let task_sink = sink.clone();
        let task = plain.spawn(async move {
            emit!(&task_sink, ProbeEvent::new(11));
        });
        let anywhere = plain.spawn_anywhere(sink.clone(), |sink| async move {
            emit!(&sink, ProbeEvent::new(12));
        });
        let blocking_sink = sink.clone();
        let blocking = plain.spawn_blocking(move || {
            emit!(&blocking_sink, ProbeEvent::new(13));
        });
        (task, anywhere, blocking)
    })
    .enrich(sink, Request { request_id: 99 })();
    futures::executor::block_on(task)?;
    futures::executor::block_on(anywhere)?;
    futures::executor::block_on(blocking)?;
    expectations.extend((11..=13).map(|value| expected(value, None)));
    assert_events(processor, &expectations);
    Ok(())
}

#[cfg(feature = "tokio")]
#[test]
fn tokio_transfers_context_for_all_spawn_methods() {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).build().unwrap();
    let _entered = runtime.enter();
    let (sink, processor) = test_emitter(TEST_ID);
    let builder = Spawner::builder_tokio(None).observed(Sink::noop()).observed(sink.clone());
    let spawner = builder.clone().build();
    assert!(format!("{builder:?}").contains(&format!("{sink:?}")));
    exercise_spawns(&spawner, &Spawner::builder_tokio(None).build(), &sink, &processor).unwrap();
}

#[cfg(feature = "tokio")]
#[test]
fn tokio_handle_transfers_context_without_an_ambient_runtime() {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).build().unwrap();
    let (sink, processor) = test_emitter(TEST_ID);
    let mut spawner = Spawner::builder_tokio(runtime.handle().clone()).observed(sink.clone()).build();
    _ = thread_aware::Relocator::between_threads().relocate(&mut spawner);
    let plain = Spawner::new_tokio(runtime.handle().clone());
    exercise_spawns(&spawner, &plain, &sink, &processor).unwrap();
}

#[cfg(feature = "tokio")]
#[test]
fn pending_task_does_not_leak_context_on_the_same_thread() {
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    runtime.block_on(async {
        let (sink, processor) = test_emitter(TEST_ID);
        let spawner = Spawner::builder_tokio(None).observed(sink.clone()).build();
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let task_sink = sink.clone();
        let task = (|| {
            spawner.spawn(async move {
                emit!(&task_sink, ProbeEvent::new(1));
                started.send(()).unwrap();
                released.await.unwrap();
                emit!(&task_sink, ProbeEvent::new(2));
            })
        })
        .enrich(&sink, Request { request_id: 42 })();

        ready.await.unwrap();
        emit!(&sink, ProbeEvent::new(3));
        let unrelated_sink = sink.clone();
        Spawner::new_tokio(None)
            .spawn(async move {
                emit!(&unrelated_sink, ProbeEvent::new(4));
            })
            .await
            .unwrap();
        release.send(()).unwrap();
        task.await.unwrap();
        emit!(&sink, ProbeEvent::new(5));

        assert_events(
            &processor,
            &[
                expected(1, Some(42)),
                expected(2, Some(42)),
                expected(3, None),
                expected(4, None),
                expected(5, None),
            ],
        );
    });
}

#[cfg(feature = "arty")]
#[test]
fn arty_transfers_context_for_all_spawn_methods() {
    let runtime = arty::runtime::Runtime::builder()
        .workers(arty::runtime::WorkersPolicy::exactly(1))
        .build()
        .unwrap();
    let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
    let (sink, processor) = test_emitter(TEST_ID);
    let builder = Spawner::builder_arty(scheduler.clone())
        .observed(Sink::noop())
        .observed(sink.clone());
    let spawner = builder.clone().build();
    assert!(format!("{builder:?}").contains(&format!("{sink:?}")));
    exercise_spawns(&spawner, &Spawner::builder_arty(scheduler).build(), &sink, &processor).unwrap();
    runtime.stop().unwrap();
}

#[cfg(feature = "arty")]
#[test]
fn arty_relocates_data_before_the_observed_factory() {
    struct Probe {
        sink: Sink,
        destination: Option<std::thread::ThreadId>,
    }

    impl thread_aware::ThreadAware for Probe {
        fn relocate(&mut self, _source: Option<&thread_aware::Thread>, destination: &thread_aware::Thread) {
            self.destination = Some(destination.id());
        }
    }

    let runtime = arty::runtime::Runtime::builder()
        .workers(arty::runtime::WorkersPolicy::exactly(1))
        .build()
        .unwrap();
    let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
    let (sink, processor) = test_emitter(TEST_ID);
    let spawner = Spawner::builder_arty(scheduler).observed(sink.clone()).build();
    let task = (|| {
        spawner.spawn_anywhere(
            Probe {
                sink: sink.clone(),
                destination: None,
            },
            |probe| {
                assert_eq!(probe.destination, Some(std::thread::current().id()));
                emit!(&probe.sink, ProbeEvent::new(1));
                async { 42 }
            },
        )
    })
    .enrich(&sink, Request { request_id: 7 })();

    assert_eq!(futures::executor::block_on(task).unwrap(), 42);
    assert_events(&processor, &[expected(1, Some(7))]);
    runtime.stop().unwrap();
}
