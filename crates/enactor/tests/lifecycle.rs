// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for runtime lifecycle control behavior.

use std::task::Poll;
use std::time::Duration;

use arty::task::Context as TaskContext;
use enactor::{Actor, ActorRef, ActorRuntime, Context};
use futures::{FutureExt, poll};

/// Upper bound for completion waits that are expected to succeed.
const COMPLETION_TIMEOUT: Duration = Duration::from_secs(5);

fn completion_timeout() -> Duration {
    COMPLETION_TIMEOUT
}

struct Block {
    started: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
}

struct Ack {
    ack: oneshot::Sender<()>,
}

enum LifecycleMessage {
    Block(Block),
    Ack(Ack),
    Panic,
    Stop,
}

#[derive(Default)]
struct LifecycleActor {
    dropped: Option<oneshot::Sender<()>>,
}

impl Drop for LifecycleActor {
    fn drop(&mut self) {
        if let Some(dropped) = self.dropped.take() {
            let _ = dropped.send(());
        }
    }
}

impl Actor for LifecycleActor {
    type Message = LifecycleMessage;

    async fn handle(&mut self, message: Self::Message, ctx: &mut Context<Self::Message>) {
        match message {
            LifecycleMessage::Block(message) => {
                let _ = message.started.send(());
                let _ = message.release.await;
            }
            LifecycleMessage::Ack(message) => {
                let _ = message.ack.send(());
            }
            #[expect(clippy::panic, reason = "the test needs an actor task that panics")]
            LifecycleMessage::Panic => panic!("completion test actor failure"),
            LifecycleMessage::Stop => ctx.stop(),
        }
    }
}

fn spawn_lifecycle_actor(rt: &ActorRuntime) -> ActorRef<LifecycleMessage> {
    rt.spawn(LifecycleActor::default())
}

fn spawn_lifecycle_actor_with_capacity(rt: &ActorRuntime, capacity: usize) -> ActorRef<LifecycleMessage> {
    rt.builder().with_capacity(capacity).spawn(LifecycleActor::default()).into()
}

async fn block_lifecycle_actor(actor: &ActorRef<LifecycleMessage>) -> oneshot::Sender<()> {
    let (started_tx, started_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    actor
        .tell(LifecycleMessage::Block(Block {
            started: started_tx,
            release: release_rx,
        }))
        .await
        .expect("failed to queue blocking message");

    started_rx.await.expect("actor never started blocking message");

    release_tx
}

async fn buffer_lifecycle_messages(actor: &ActorRef<LifecycleMessage>, count: usize) -> Vec<oneshot::Receiver<()>> {
    let mut buffered = Vec::with_capacity(count);

    for _ in 0..count {
        let (ack_tx, ack_rx) = oneshot::channel();
        actor
            .tell(LifecycleMessage::Ack(Ack { ack: ack_tx }))
            .await
            .expect("failed to queue buffered message");
        buffered.push(ack_rx);
    }

    buffered
}

#[arty::test]
async fn runtime_stop_skips_buffered_messages(cx: TaskContext) {
    let rt = ActorRuntime::from_builtins(&cx);
    let actor = spawn_lifecycle_actor(&rt);

    let (started_tx, started_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    actor
        .tell(LifecycleMessage::Block(Block {
            started: started_tx,
            release: release_rx,
        }))
        .await
        .expect("failed to queue blocking message");

    started_rx.await.expect("actor never started blocking message");

    let (queued_tx, queued_rx) = oneshot::channel();
    actor
        .tell(LifecycleMessage::Ack(Ack { ack: queued_tx }))
        .await
        .expect("failed to queue buffered message");

    actor.stop().await.expect("stop failed");
    release_tx.send(()).expect("release should succeed");

    let (late_tx, _late_rx) = oneshot::channel();
    actor
        .tell(LifecycleMessage::Ack(Ack { ack: late_tx }))
        .await
        .expect_err("stopped actor must reject new messages");

    drop(actor);
    queued_rx.await.expect_err("buffered message should be dropped after stop");
}

#[arty::test]
async fn runtime_drain_processes_buffered_messages(cx: TaskContext) {
    let rt = ActorRuntime::from_builtins(&cx);
    let actor = spawn_lifecycle_actor(&rt);

    let (first_tx, first_rx) = oneshot::channel();
    actor
        .tell(LifecycleMessage::Ack(Ack { ack: first_tx }))
        .await
        .expect("failed to queue first message");

    let (second_tx, second_rx) = oneshot::channel();
    actor
        .tell(LifecycleMessage::Ack(Ack { ack: second_tx }))
        .await
        .expect("failed to queue second message");

    actor.drain().await.expect("drain failed");

    first_rx.await.expect("first buffered message was not drained");
    second_rx.await.expect("second buffered message was not drained");

    let (late_tx, _late_rx) = oneshot::channel();
    actor
        .tell(LifecycleMessage::Ack(Ack { ack: late_tx }))
        .await
        .expect_err("draining actor must reject new messages");
}

#[arty::test]
async fn explicit_capacity_applies_backpressure(cx: TaskContext) {
    const CAPACITY: usize = 2;

    let rt = ActorRuntime::from_builtins(&cx);
    let actor = spawn_lifecycle_actor_with_capacity(&rt, CAPACITY);
    let release_tx = block_lifecycle_actor(&actor).await;
    let buffered = buffer_lifecycle_messages(&actor, CAPACITY).await;
    let (backpressured_tx, backpressured_rx) = oneshot::channel();
    let mut backpressured = Box::pin(actor.tell(LifecycleMessage::Ack(Ack { ack: backpressured_tx })));

    assert!(matches!(poll!(&mut backpressured), Poll::Pending));

    release_tx.send(()).expect("release should succeed");
    backpressured
        .await
        .expect("backpressured message should send after capacity is available");
    actor.drain().await.expect("drain should succeed");

    for ack in buffered {
        ack.await.expect("buffered message should be handled");
    }
    backpressured_rx.await.expect("backpressured message should be handled");
}

#[arty::test]
async fn completion_waits_for_handler_exit_after_stop(cx: TaskContext) {
    let runtime = ActorRuntime::from_builtins(&cx);
    let (dropped, mut actor_dropped) = oneshot::channel();
    let spawned = runtime.builder().spawn(LifecycleActor { dropped: Some(dropped) });
    let release = block_lifecycle_actor(spawned.actor()).await;
    let mut buffered = buffer_lifecycle_messages(spawned.actor(), 1).await;

    spawned.actor().stop().await.expect("stop is admitted");
    let mut completion = Box::pin(spawned.wait_for_completion(cx.clock(), completion_timeout()));
    assert!(matches!(poll!(&mut completion), Poll::Pending));
    drop(completion);

    let observer = spawned.clone();
    let mut another_wait = Box::pin(observer.wait_for_completion(cx.clock(), completion_timeout()));
    assert!(matches!(poll!(&mut another_wait), Poll::Pending));
    release.send(()).expect("handler is still running");
    another_wait.await.expect("observer sees completion");
    spawned
        .wait_for_completion(cx.clock(), Duration::ZERO)
        .await
        .expect("completed task wins over zero timeout");

    assert!(matches!(poll!(&mut actor_dropped), Poll::Ready(Ok(()))));
    drop(observer);
    drop(spawned);
    buffered
        .pop()
        .expect("one buffered message")
        .await
        .expect_err("stop does not process buffered messages");
}

#[arty::test]
async fn completion_waits_for_buffered_messages_after_drain(cx: TaskContext) {
    let runtime = ActorRuntime::from_builtins(&cx);
    let spawned = runtime.builder().spawn(LifecycleActor::default());
    let release = block_lifecycle_actor(spawned.actor()).await;
    let mut buffered = buffer_lifecycle_messages(spawned.actor(), 2).await;

    spawned.actor().drain().await.expect("drain is admitted");
    let mut completion = Box::pin(spawned.wait_for_completion(cx.clock(), completion_timeout()));
    assert!(matches!(poll!(&mut completion), Poll::Pending));
    release.send(()).expect("handler is still running");
    completion.await.expect("drain completes");

    for acknowledgement in &mut buffered {
        assert!(matches!(poll!(acknowledgement), Poll::Ready(Ok(()))));
    }
}

#[arty::test]
async fn completion_observes_stream_actor_self_stop(cx: TaskContext) {
    let runtime = ActorRuntime::from_builtins(&cx);
    let (dropped, mut actor_dropped) = oneshot::channel();
    let spawned = runtime
        .builder()
        .with_stream(futures::stream::iter([LifecycleMessage::Stop]))
        .spawn(LifecycleActor { dropped: Some(dropped) });

    spawned
        .wait_for_completion(cx.clock(), completion_timeout())
        .await
        .expect("self-stopping actor completes");

    assert!(matches!(poll!(&mut actor_dropped), Poll::Ready(Ok(()))));
}

// Keep the runtime running after the actor panic so the waiter can observe it.
#[arty::test(panic = ignore)]
async fn completion_propagates_actor_panic(cx: TaskContext) {
    let runtime = ActorRuntime::from_builtins(&cx);
    let spawned = runtime.builder().spawn(LifecycleActor::default());
    spawned
        .actor()
        .tell(LifecycleMessage::Panic)
        .await
        .expect("panic message is admitted");

    let failure = std::panic::AssertUnwindSafe(spawned.wait_for_completion(cx.clock(), completion_timeout()))
        .catch_unwind()
        .await
        .expect_err("actor panic must not report successful completion");

    assert_eq!(failure.downcast_ref::<&str>(), Some(&"completion test actor failure"));
}

#[arty::test]
async fn completion_timeout_preserves_task_and_independent_waiter(cx: TaskContext) {
    let runtime = ActorRuntime::from_builtins(&cx);
    let (dropped, mut actor_dropped) = oneshot::channel();
    let spawned = runtime.builder().spawn(LifecycleActor { dropped: Some(dropped) });
    let release = block_lifecycle_actor(spawned.actor()).await;
    let observer = spawned.clone();
    let mut longer_wait = Box::pin(observer.wait_for_completion(cx.clock(), completion_timeout()));
    assert!(poll!(longer_wait.as_mut()).is_pending());

    let error = spawned
        .wait_for_completion(cx.clock(), Duration::ZERO)
        .await
        .expect_err("blocked actor must time out");
    assert_eq!(error.timeout, Duration::ZERO);
    assert!(poll!(&mut actor_dropped).is_pending());
    assert!(poll!(longer_wait.as_mut()).is_pending());

    let buffered = buffer_lifecycle_messages(spawned.actor(), 1).await;
    spawned.actor().drain().await.expect("drain is admitted");
    release.send(()).expect("timeout did not cancel the handler");
    longer_wait.await.expect("other observer still completes");
    spawned
        .wait_for_completion(cx.clock(), Duration::ZERO)
        .await
        .expect("wait can be retried after timeout");
    for acknowledgement in buffered {
        acknowledgement.await.expect("buffered work was processed");
    }
    assert!(poll!(&mut actor_dropped).is_ready());
}

#[arty::test]
async fn completion_timeout_uses_configured_duration(cx: TaskContext) {
    let runtime = ActorRuntime::from_builtins(&cx);
    let spawned = runtime.builder().spawn(LifecycleActor::default());
    let release = block_lifecycle_actor(spawned.actor()).await;
    let control = tick::ClockControl::new().auto_advance_timers(true);
    let clock = control.to_clock();

    for timeout in [Duration::from_secs(7), Duration::from_secs(19)] {
        let started = clock.instant();
        let error = spawned
            .wait_for_completion(&clock, timeout)
            .await
            .expect_err("blocked task must exhaust the configured timeout");
        assert_eq!(error.timeout, timeout);
        assert_eq!(error.to_string(), format!("completion wait timed out after {timeout:?}"));
        assert_eq!(clock.instant().duration_since(started), timeout);
    }

    spawned.actor().stop().await.expect("stop is admitted");
    release.send(()).expect("timed out task is still running");
    spawned
        .wait_for_completion(cx.clock(), completion_timeout())
        .await
        .expect("task completes after release");
}

#[arty::test]
async fn runtime_drain_spam_preserves_buffered_messages(cx: TaskContext) {
    const BUFFERED_MESSAGES: usize = 256;
    const DRAIN_REQUESTS: usize = 512;

    let rt = ActorRuntime::from_builtins(&cx);
    let actor = spawn_lifecycle_actor_with_capacity(&rt, BUFFERED_MESSAGES + 1);

    let release_tx = block_lifecycle_actor(&actor).await;
    let buffered = buffer_lifecycle_messages(&actor, BUFFERED_MESSAGES).await;

    for _ in 0..DRAIN_REQUESTS {
        actor.drain().await.expect("drain failed");
    }

    release_tx.send(()).expect("release should succeed");

    for ack in buffered {
        ack.await.expect("buffered message was not drained after control spam");
    }

    let (late_tx, _late_rx) = oneshot::channel();
    actor
        .tell(LifecycleMessage::Ack(Ack { ack: late_tx }))
        .await
        .expect_err("draining actor must reject new messages after control spam");
}

#[arty::test]
async fn runtime_stop_spam_drops_buffered_messages(cx: TaskContext) {
    const BUFFERED_MESSAGES: usize = 256;
    const STOP_REQUESTS: usize = 512;

    let rt = ActorRuntime::from_builtins(&cx);
    let actor = spawn_lifecycle_actor_with_capacity(&rt, BUFFERED_MESSAGES + 1);

    let release_tx = block_lifecycle_actor(&actor).await;
    let buffered = buffer_lifecycle_messages(&actor, BUFFERED_MESSAGES).await;

    for _ in 0..STOP_REQUESTS {
        actor.stop().await.expect("stop failed");
    }

    release_tx.send(()).expect("release should succeed");

    let (late_tx, _late_rx) = oneshot::channel();
    actor
        .tell(LifecycleMessage::Ack(Ack { ack: late_tx }))
        .await
        .expect_err("stopped actor must reject new messages after control spam");

    drop(actor);

    for ack in buffered {
        ack.await.expect_err("buffered message should be dropped after stop spam");
    }
}
