// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for how an actor reports its exit.

use std::panic::AssertUnwindSafe;
use std::time::Duration;

use arty::runtime::Runtime;
use arty::runtime::policy::WorkersPolicy;
use arty::task::Context as TaskContext;
use enactor::{Actor, ActorExit, ActorRuntime, Context, Placement, PlacementToken};
use futures::FutureExt;

/// Upper bound for completion waits that are expected to succeed.
const COMPLETION_TIMEOUT: Duration = Duration::from_secs(5);

const PANIC_MESSAGE: &str = "exit test actor failure";

enum Command {
    Stop,
    Drain,
    Panic,
}

struct ExitActor;

impl Actor for ExitActor {
    type Message = Command;

    async fn handle(&mut self, command: Command, ctx: &mut Context<Command>) {
        match command {
            Command::Stop => ctx.stop(),
            Command::Drain => ctx.drain(),
            #[expect(clippy::panic, reason = "the test needs a handler that panics")]
            Command::Panic => panic!("{PANIC_MESSAGE}"),
        }
    }
}

fn single_worker_runtime() -> Runtime {
    Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .build()
        .expect("one worker is a valid runtime configuration")
}

#[arty::test]
async fn external_stop_reports_stopped(cx: TaskContext) {
    let spawned = ActorRuntime::from_builtins(&cx).builder().spawn(ExitActor);

    spawned.actor().stop().await.unwrap();

    assert_eq!(spawned.exit().await, ActorExit::Stopped);
}

#[arty::test]
async fn external_drain_reports_drained(cx: TaskContext) {
    let spawned = ActorRuntime::from_builtins(&cx).builder().spawn(ExitActor);

    spawned.actor().drain().await.unwrap();

    assert_eq!(spawned.exit().await, ActorExit::Drained);
}

#[arty::test]
async fn context_stop_reports_stopped(cx: TaskContext) {
    let spawned = ActorRuntime::from_builtins(&cx).builder().spawn(ExitActor);

    spawned.actor().tell(Command::Stop).await.unwrap();

    assert_eq!(spawned.exit().await, ActorExit::Stopped);
}

#[arty::test]
async fn context_drain_reports_drained(cx: TaskContext) {
    let spawned = ActorRuntime::from_builtins(&cx).builder().spawn(ExitActor);

    spawned.actor().tell(Command::Drain).await.unwrap();

    assert_eq!(spawned.exit().await, ActorExit::Drained);
}

#[arty::test(panic = ignore)]
async fn handler_panic_reports_panicked(cx: TaskContext) {
    let spawned = ActorRuntime::from_builtins(&cx).builder().spawn(ExitActor);

    spawned.actor().tell(Command::Panic).await.unwrap();

    assert_eq!(spawned.exit().await, ActorExit::Panicked);
}

#[arty::test(panic = ignore)]
async fn exit_leaves_the_panic_payload_for_the_first_completion_waiter(cx: TaskContext) {
    let spawned = ActorRuntime::from_builtins(&cx).builder().spawn(ExitActor);
    spawned.actor().tell(Command::Panic).await.unwrap();
    assert_eq!(spawned.exit().await, ActorExit::Panicked);

    let payload = AssertUnwindSafe(spawned.wait_for_completion(cx.clock(), COMPLETION_TIMEOUT))
        .catch_unwind()
        .await
        .unwrap_err();

    assert_eq!(payload.downcast_ref::<String>().map(String::as_str), Some(PANIC_MESSAGE));
}

#[arty::test(panic = ignore)]
async fn later_completion_waiter_panics_with_a_message(cx: TaskContext) {
    let spawned = ActorRuntime::from_builtins(&cx).builder().spawn(ExitActor);
    spawned.actor().tell(Command::Panic).await.unwrap();
    let first = spawned.clone();
    AssertUnwindSafe(first.wait_for_completion(cx.clock(), COMPLETION_TIMEOUT))
        .catch_unwind()
        .await
        .unwrap_err();

    let payload = AssertUnwindSafe(spawned.wait_for_completion(cx.clock(), COMPLETION_TIMEOUT))
        .catch_unwind()
        .await
        .unwrap_err();

    assert_eq!(
        payload.downcast_ref::<&str>(),
        Some(&"the actor panicked, and another waiter received the panic payload")
    );
}

#[arty::test]
async fn clones_and_late_waiters_see_the_same_exit(cx: TaskContext) {
    let spawned = ActorRuntime::from_builtins(&cx).builder().spawn(ExitActor);
    let clone = spawned.clone();
    let early = clone.exit();

    spawned.actor().drain().await.unwrap();
    let early = early.await;
    let late = spawned.exit().await;

    assert_eq!(
        (early, late, clone.exit().await),
        (ActorExit::Drained, ActorExit::Drained, ActorExit::Drained)
    );
}

#[test]
fn placement_on_another_runtime_reports_aborted() {
    let home = single_worker_runtime();
    let other = single_worker_runtime();
    let foreign = other.scheduler().block_on(async |cx| PlacementToken::from(cx.thread())).unwrap();

    let exit = home
        .scheduler()
        .block_on(async move |cx| {
            let spawned = ActorRuntime::from_builtins(&cx)
                .builder()
                .with_meta(Placement::SameThreadAs(foreign))
                .spawn(ExitActor);
            spawned.exit().await
        })
        .unwrap();

    assert_eq!(exit, ActorExit::Aborted);
    home.stop().unwrap();
    other.stop().unwrap();
}

#[test]
fn runtime_stop_reports_aborted() {
    let runtime = single_worker_runtime();
    let spawned = runtime
        .scheduler()
        .block_on(async |cx| ActorRuntime::from_builtins(&cx).builder().spawn(ExitActor))
        .unwrap();

    runtime.stop().unwrap();

    assert_eq!(futures::executor::block_on(spawned.exit()), ActorExit::Aborted);
}
