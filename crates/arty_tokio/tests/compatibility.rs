// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests Tokio behavior through the public extension API.

#![expect(clippy::unwrap_used, reason = "test failures use unwrap for concise backtraces")]

mod support;

use arty::runtime::{Runtime, WorkersPolicy};
use arty::task::Context;
use arty_tokio::TokioCompatExt as _;
use static_assertions::assert_impl_all;
use support::stop_bounded;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::runtime::{Builder, Handle};

assert_impl_all!(arty_tokio::TokioHandle: Send, Sync, Default);

async fn worker_runtime_ids(cx: &Context) -> Vec<String> {
    let tasks = cx
        .control()
        .workers()
        .into_iter()
        .map(|worker| {
            cx.scheduler()
                .spawn_at(worker, (), |()| async { Handle::current().id().to_string() })
        })
        .collect::<Vec<_>>();
    let mut contexts = Vec::with_capacity(tasks.len());
    for task in tasks {
        contexts.push(task.await.unwrap());
    }
    contexts
}

#[test]
fn builder_supports_tokio_on_every_worker() {
    let tokio = Builder::new_multi_thread().enable_all().build().unwrap();
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(2))
        .tokio_compat_with(tokio.handle())
        .build()
        .unwrap();

    let observations = runtime
        .scheduler()
        .block_on(async |cx| {
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
fn repeated_support_for_the_same_runtime_is_idempotent() {
    let tokio = Builder::new_multi_thread().enable_all().build().unwrap();
    let runtime = Runtime::builder()
        .tokio_compat_with(tokio.handle())
        .tokio_compat_with(tokio.handle())
        .build()
        .unwrap();

    let value = runtime
        .scheduler()
        .block_on(async |_| tokio::spawn(async { 42 }).await.unwrap())
        .unwrap();

    assert_eq!(value, 42);
    stop_bounded(runtime);
}

#[test]
fn first_builder_initialization_wins_with_different_runtimes() {
    let first = Builder::new_multi_thread().worker_threads(1).build().unwrap();
    let second = Builder::new_multi_thread().worker_threads(1).build().unwrap();
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(2))
        .tokio_compat_with(first.handle())
        .tokio_compat_with(second.handle())
        .build()
        .unwrap();

    let ids = runtime.scheduler().block_on(async |cx| worker_runtime_ids(&cx).await).unwrap();

    assert_eq!(ids, vec![first.handle().id().to_string(); 2]);
    stop_bounded(runtime);
}

#[test]
fn first_control_initialization_wins_with_different_runtimes() {
    let first = Builder::new_multi_thread().worker_threads(1).build().unwrap();
    let second = Builder::new_current_thread().build().unwrap();
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(2)).build().unwrap();

    let ids = runtime
        .scheduler()
        .block_on(async |cx| {
            cx.control().tokio_compat_with(first.handle());
            cx.control().tokio_compat_with(second.handle());
            worker_runtime_ids(&cx).await
        })
        .unwrap();

    assert_eq!(ids, vec![first.handle().id().to_string(); 2]);
    stop_bounded(runtime);
}

#[test]
fn owned_default_runtime_supports_tokio_tasks() {
    let runtime = Runtime::builder().tokio_compat().build().unwrap();

    let value = runtime
        .scheduler()
        .block_on(async |_| tokio::spawn(async { 42 }).await.unwrap())
        .unwrap();

    assert_eq!(value, 42);
    stop_bounded(runtime);
}

#[test]
fn owned_custom_runtime_supports_tokio_tasks() {
    let tokio = Builder::new_multi_thread().worker_threads(2).build().unwrap();
    let expected_runtime = tokio.handle().id().to_string();
    let runtime = Runtime::builder()
        .tokio_compat_with(arty_tokio::TokioHandle::from(tokio))
        .build()
        .unwrap();

    let (actual_runtime, value) = runtime
        .scheduler()
        .block_on(async |_| {
            let runtime = Handle::current().id().to_string();
            (runtime, tokio::spawn(async { 42 }).await.unwrap())
        })
        .unwrap();

    assert_eq!(actual_runtime, expected_runtime);
    assert_eq!(value, 42);
    stop_bounded(runtime);
}

#[test]
fn runtime_control_supports_tokio_on_every_worker() {
    let tokio = Builder::new_multi_thread().enable_all().build().unwrap();
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(2)).build().unwrap();

    let contexts = runtime
        .scheduler()
        .block_on(async |cx| {
            cx.control().tokio_compat_with(tokio.handle());
            worker_runtime_ids(&cx).await
        })
        .unwrap();

    assert_eq!(contexts, vec![tokio.handle().id().to_string(); 2]);
    stop_bounded(runtime);
}

#[test]
fn tokio_compat_includes_tasks_and_in_memory_io() {
    let tokio = Builder::new_multi_thread().enable_all().build().unwrap();
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(2))
        .tokio_compat_with(tokio.handle())
        .build()
        .unwrap();

    let response = runtime
        .scheduler()
        .block_on(async |_| {
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
}

#[test]
fn tokio_join_handles_keep_native_panic_semantics() {
    let tokio = Builder::new_multi_thread().enable_all().build().unwrap();
    let runtime = Runtime::builder().tokio_compat_with(tokio.handle()).build().unwrap();

    let error = runtime
        .scheduler()
        .block_on(async |_| tokio::spawn(async { panic!("Tokio task panic") }).await.unwrap_err())
        .unwrap();

    assert!(error.is_panic());
    stop_bounded(runtime);
}

#[test]
fn tokio_handle_debug_is_opaque() {
    let tokio = Builder::new_multi_thread().build().unwrap();
    assert_eq!(format!("{:?}", arty_tokio::TokioHandle::from(tokio.handle())), "TokioHandle { .. }");
}

#[test]
fn current_thread_runtime_works_when_the_application_drives_it() {
    let tokio = Builder::new_current_thread().enable_all().build().unwrap();
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .tokio_compat_with(tokio.handle())
        .build()
        .unwrap();
    let task = runtime
        .scheduler()
        .spawn_anywhere((), |_, ()| async { tokio::spawn(async { 42 }).await.unwrap() });

    assert_eq!(tokio.block_on(task).unwrap(), 42);
    stop_bounded(runtime);
}
