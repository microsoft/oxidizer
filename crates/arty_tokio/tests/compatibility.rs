// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests Tokio behavior through the public compatibility API.

#![expect(clippy::unwrap_used, reason = "test failures use unwrap for concise backtraces")]

mod support;

use std::error::Error as _;
use std::future::{poll_fn, ready};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Instant;

use arty::core::{Thread, ThreadAware};
use arty::task::{Builtins, Scheduler};
use performables::sync::channel::{OneshotReceiver, OneshotSender, oneshot};
use static_assertions::assert_impl_all;
use support::{TEST_TIMEOUT, runtime, stop_bounded};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::runtime::{Builder, Handle};

assert_impl_all!(arty_tokio::TokioHandle: Send, Sync);
assert_impl_all!(arty_tokio::InitError: std::error::Error, Send, Sync);

async fn worker_contexts(cx: &Builtins) -> Vec<bool> {
    let tasks = cx
        .control()
        .workers()
        .into_iter()
        .map(|worker| cx.scheduler().spawn_at(worker, (), |()| async { Handle::try_current().is_ok() }))
        .collect::<Vec<_>>();
    let mut contexts = Vec::with_capacity(tasks.len());
    for task in tasks {
        contexts.push(task.await.unwrap());
    }
    contexts
}

fn worker_has_context(scheduler: &Scheduler, worker: &Thread) -> bool {
    futures::executor::block_on(scheduler.spawn_at(worker, (), |()| async { Handle::try_current().is_ok() })).unwrap()
}

#[test]
fn default_init_makes_tokio_available_on_every_worker() {
    let runtime = runtime(2);

    let observations = runtime
        .scheduler()
        .block_on(async |cx| {
            arty_tokio::init(&cx, arty_tokio::TokioHandle::default()).await.unwrap();
            let workers = cx.control().workers().into_iter().cloned().collect::<Vec<_>>();
            let tasks = workers
                .iter()
                .map(|worker| {
                    cx.scheduler().spawn_at(worker, (), |()| async {
                        let current = Handle::try_current().is_ok();
                        let spawned = tokio::spawn(async { 40 + 2 }).await.unwrap();
                        let blocking = tokio::task::spawn_blocking(|| 6 * 7).await.unwrap();
                        (current, spawned, blocking)
                    })
                })
                .collect::<Vec<_>>();

            let mut observations = Vec::with_capacity(tasks.len());
            for task in tasks {
                observations.push(task.await.unwrap());
            }
            observations
        })
        .unwrap();

    assert_eq!(observations, vec![(true, 42, 42); 2]);
    stop_bounded(runtime);
}

#[test]
fn duplicate_init_rolls_back_without_removing_existing_contexts() {
    let runtime = runtime(2);

    let (error, contexts) = runtime
        .scheduler()
        .block_on(async |cx| {
            arty_tokio::init(&cx, arty_tokio::TokioHandle::default()).await.unwrap();
            let error = arty_tokio::init(&cx, arty_tokio::TokioHandle::default()).await.unwrap_err();
            (error, worker_contexts(&cx).await)
        })
        .unwrap();

    assert_eq!(error.to_string(), "an Arty worker already has a Tokio runtime context");
    assert_eq!(contexts, vec![true, true]);
    stop_bounded(runtime);
}

#[test]
fn external_runtime_supports_tokio_tasks_and_in_memory_io() {
    let tokio = Builder::new_multi_thread().enable_all().build().unwrap();
    let runtime = runtime(2);

    let response = runtime
        .scheduler()
        .block_on(async |cx| {
            arty_tokio::init(&cx, tokio.handle()).await.unwrap();
            let (mut client, mut server) = tokio::io::duplex(4);
            let server = tokio::spawn(async move {
                let mut request = [0; 4];
                server.read_exact(&mut request).await.unwrap();
                assert_eq!(&request, b"ping");
                server.write_all(b"pong").await.unwrap();
            });

            client.write_all(b"ping").await.unwrap();
            let mut response = [0; 4];
            client.read_exact(&mut response).await.unwrap();
            server.await.unwrap();
            response
        })
        .unwrap();

    assert_eq!(&response, b"pong");
    stop_bounded(runtime);
    drop(tokio);
}

#[test]
fn tokio_join_handles_keep_native_panic_semantics() {
    let runtime = runtime(1);

    let error = runtime
        .scheduler()
        .block_on(async |cx| {
            arty_tokio::init(&cx, arty_tokio::TokioHandle::default()).await.unwrap();
            tokio::spawn(async { panic!("Tokio task panic") }).await.unwrap_err()
        })
        .unwrap();

    assert!(error.is_panic());
    stop_bounded(runtime);
}

#[test]
fn tokio_handle_debug_is_opaque() {
    assert_eq!(format!("{:?}", arty_tokio::TokioHandle::default()), "TokioHandle { .. }");
}

fn current_thread_init_error() -> arty_tokio::InitError {
    let tokio = Builder::new_current_thread().enable_all().build().unwrap();
    let runtime = runtime(1);

    let error = runtime
        .scheduler()
        .block_on(async |cx| arty_tokio::init(&cx, tokio.handle()).await.unwrap_err())
        .unwrap();

    stop_bounded(runtime);
    drop(tokio);
    error
}

#[test]
fn current_thread_runtime_is_rejected() {
    assert_eq!(
        current_thread_init_error().to_string(),
        "Tokio compatibility requires a multi-thread Tokio runtime"
    );
}

#[test]
fn init_error_debug_is_opaque() {
    assert_eq!(format!("{:?}", current_thread_init_error()), "InitError { .. }");
}

#[test]
fn init_error_preserves_its_private_source() {
    assert!(current_thread_init_error().source().is_some());
}

#[test]
fn shutdown_during_submission_is_a_typed_init_error() {
    let runtime = runtime(1);

    let error = runtime
        .scheduler()
        .block_on(async |cx| {
            cx.control().request_stop();
            arty_tokio::init(&cx, arty_tokio::TokioHandle::default()).await.unwrap_err()
        })
        .unwrap();

    assert_eq!(
        error.to_string(),
        "Arty shut down during Tokio compatibility initialization\ncaused by: runtime is shutting down"
    );
    stop_bounded(runtime);
}

struct ExistingContext {
    handle: Handle,
    ready: OneshotSender<()>,
    release: OneshotReceiver<()>,
}

impl ThreadAware for ExistingContext {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

#[expect(
    clippy::future_not_send,
    reason = "the fixed-worker test task must retain Tokio's non-Send EnterGuard"
)]
async fn hold_existing_context(data: ExistingContext) {
    let _entered = data.handle.enter();
    data.ready.send(()).unwrap();
    data.release.await.unwrap();
}

struct WorkerBlocker {
    started: mpsc::SyncSender<()>,
    release: mpsc::Receiver<()>,
}

impl ThreadAware for WorkerBlocker {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "Scheduler::spawn_at requires a function that takes owned relocated data"
)]
fn block_worker(data: WorkerBlocker) -> std::future::Ready<()> {
    data.started.send(()).unwrap();
    data.release.recv().unwrap();
    ready(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InitOutcome {
    Success,
    Failed,
}

impl ThreadAware for InitOutcome {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

#[test]
fn partial_conflict_rolls_back_and_allows_retry() {
    let tokio = Builder::new_multi_thread().enable_all().build().unwrap();
    let runtime = runtime(2);
    let (builtins, scheduler, workers) = runtime
        .scheduler()
        .block_on(async |cx| {
            let workers = cx.control().workers().into_iter().cloned().collect::<Vec<_>>();
            (cx.clone(), cx.scheduler().clone(), workers)
        })
        .unwrap();
    let clean_worker = workers[0].clone();
    let conflict_worker = workers[1].clone();

    let (context_ready, context_started) = oneshot();
    let (context_release, context_gate) = oneshot();
    let holder = scheduler.spawn_at(
        &conflict_worker,
        ExistingContext {
            handle: tokio.handle().clone(),
            ready: context_ready,
            release: context_gate,
        },
        hold_existing_context,
    );
    futures::executor::block_on(context_started).unwrap();

    let (blocker_started, blocker_ready) = mpsc::sync_channel(1);
    let (blocker_release, blocker_gate) = mpsc::sync_channel(1);
    let blocker = scheduler.spawn_at(
        &conflict_worker,
        WorkerBlocker {
            started: blocker_started,
            release: blocker_gate,
        },
        block_worker,
    );
    blocker_ready.recv_timeout(TEST_TIMEOUT).unwrap();

    let initialization = scheduler.spawn_at(&clean_worker, builtins.clone(), |cx| async move {
        match arty_tokio::init(&cx, arty_tokio::TokioHandle::default()).await {
            Ok(()) => InitOutcome::Success,
            Err(error) => {
                assert_eq!(error.to_string(), "an Arty worker already has a Tokio runtime context");
                InitOutcome::Failed
            }
        }
    });
    let deadline = Instant::now() + TEST_TIMEOUT;
    while !worker_has_context(&scheduler, &clean_worker) {
        assert!(Instant::now() < deadline, "clean worker did not prepare its Tokio context");
        std::thread::yield_now();
    }

    blocker_release.send(()).unwrap();
    futures::executor::block_on(blocker).unwrap();
    let outcome = futures::executor::block_on(initialization).unwrap();
    assert_eq!(outcome, InitOutcome::Failed);
    assert!(!worker_has_context(&scheduler, &clean_worker));
    assert!(worker_has_context(&scheduler, &conflict_worker));

    context_release.send(()).unwrap();
    futures::executor::block_on(holder).unwrap();
    let retry = scheduler.spawn_at(&clean_worker, builtins, |cx| async move {
        arty_tokio::init(&cx, arty_tokio::TokioHandle::default()).await.is_ok()
    });
    assert!(futures::executor::block_on(retry).unwrap());

    stop_bounded(runtime);
    drop(tokio);
}

#[test]
fn cancelled_init_rolls_back_and_allows_retry() {
    let runtime = runtime(2);

    runtime
        .scheduler()
        .block_on(async |cx| {
            let mut initialization = Box::pin(arty_tokio::init(&cx, arty_tokio::TokioHandle::default()));
            poll_fn(|context| match initialization.as_mut().poll(context) {
                std::task::Poll::Pending => std::task::Poll::Ready(()),
                std::task::Poll::Ready(result) => {
                    panic!("initialization completed before cancellation: {result:?}")
                }
            })
            .await;

            assert_eq!(worker_contexts(&cx).await, vec![true, true]);
            drop(initialization);

            let deadline = Instant::now() + TEST_TIMEOUT;
            while worker_contexts(&cx).await != vec![false, false] {
                assert!(Instant::now() < deadline, "canceled initialization did not clean up");
            }

            arty_tokio::init(&cx, arty_tokio::TokioHandle::default()).await.unwrap();
            assert_eq!(worker_contexts(&cx).await, vec![true, true]);
        })
        .unwrap();

    stop_bounded(runtime);
}

#[test]
fn independent_arty_runtimes_have_independent_default_tokio_runtimes() {
    let first = runtime(1);
    let second = runtime(1);
    let completed = Arc::new(AtomicUsize::new(0));

    for runtime in [&first, &second] {
        let completed = Arc::clone(&completed);
        runtime
            .scheduler()
            .block_on(async |cx: Builtins| {
                arty_tokio::init(&cx, arty_tokio::TokioHandle::default()).await.unwrap();
                tokio::spawn(async move {
                    completed.fetch_add(1, Ordering::SeqCst);
                })
                .await
                .unwrap();
            })
            .unwrap();
    }

    assert_eq!(completed.load(Ordering::SeqCst), 2);
    stop_bounded(first);
    stop_bounded(second);
}
