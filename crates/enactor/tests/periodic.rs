// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::future::poll_fn;
use std::task::Poll;
use std::time::Duration;

use arty::task::Context as TaskContext;
use enactor::{Actor, ActorRef, ActorRuntime, Context};
use flume as mpsc;
use futures::stream;
use tick::ClockControl;

/// Lets other tasks on this worker run once.
async fn yield_now() {
    let mut yielded = false;
    poll_fn(|cx| {
        if yielded {
            Poll::Ready(())
        } else {
            yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    })
    .await;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TestMessage {
    Mailbox,
    Stream,
    FastTick,
    SlowTick,
}

#[derive(Debug)]
struct SinkActor {
    tx: mpsc::Sender<TestMessage>,
}

impl Actor for SinkActor {
    type Message = TestMessage;

    async fn handle(&mut self, message: Self::Message, _ctx: &mut Context<Self::Message>) {
        self.tx
            .send_async(message)
            .await
            .expect("sink receiver should stay open during the test");
    }
}

#[expect(clippy::panic, reason = "test helper reports a missing message as a test failure")]
async fn yield_until_received(_cx: &TaskContext, rx: &mpsc::Receiver<TestMessage>) -> TestMessage {
    for _ in 0..100 {
        if let Ok(message) = rx.try_recv() {
            return message;
        }
        yield_now().await;
    }

    panic!("actor did not deliver a message");
}

async fn prime_actor(cx: &TaskContext, actor: &ActorRef<TestMessage>, rx: &mpsc::Receiver<TestMessage>) {
    actor.tell(TestMessage::Mailbox).await.expect("mailbox message should send");
    assert_eq!(yield_until_received(cx, rx).await, TestMessage::Mailbox);

    yield_runtime(cx).await;
}

async fn yield_runtime(_cx: &TaskContext) {
    for _ in 0..32 {
        yield_now().await;
    }
}

async fn receive_many(cx: &TaskContext, rx: &mpsc::Receiver<TestMessage>, count: usize) -> Vec<TestMessage> {
    let mut messages = Vec::with_capacity(count);
    for _ in 0..count {
        messages.push(yield_until_received(cx, rx).await);
    }
    messages
}

#[arty::test]
async fn periodic_message_uses_the_actor_mailbox(cx: TaskContext) {
    let clock_control = ClockControl::new();
    let clock = clock_control.clone().to_clock();
    let runtime = ActorRuntime::from_builtins(&cx);
    let (tx, rx) = mpsc::bounded(16);
    let actor = runtime.builder().with_capacity(16).spawn(SinkActor { tx });
    runtime.send_interval(&actor, &clock, Duration::from_secs(1), || TestMessage::FastTick);
    let actor: ActorRef<TestMessage> = actor.into();
    yield_runtime(&cx).await;

    for _ in 0..8 {
        actor.try_tell(TestMessage::Mailbox).expect("mailbox message should queue");
    }
    clock_control.advance(Duration::from_secs(1));

    let delivered = receive_many(&cx, &rx, 9).await;
    assert_eq!(delivered.iter().filter(|message| **message == TestMessage::FastTick).count(), 1);

    actor.stop().await.expect("actor should stop cleanly");
}

#[arty::test]
async fn missed_periodic_ticks_coalesce_without_a_catch_up_burst(cx: TaskContext) {
    let clock_control = ClockControl::new();
    let clock = clock_control.clone().to_clock();
    let runtime = ActorRuntime::from_builtins(&cx);
    let (tx, rx) = mpsc::bounded(4);
    let actor = runtime.builder().spawn(SinkActor { tx });
    runtime.send_interval(&actor, &clock, Duration::from_secs(1), || TestMessage::FastTick);
    let actor: ActorRef<TestMessage> = actor.into();
    prime_actor(&cx, &actor, &rx).await;

    clock_control.advance(Duration::from_secs(10));
    actor
        .tell(TestMessage::Mailbox)
        .await
        .expect("mailbox message should wake the actor");
    let delivered = receive_many(&cx, &rx, 2).await;
    assert!(delivered.contains(&TestMessage::FastTick));
    assert!(delivered.contains(&TestMessage::Mailbox));

    yield_runtime(&cx).await;
    assert!(rx.try_recv().is_err(), "missed ticks should coalesce");

    actor.stop().await.expect("actor should stop cleanly");
}

#[arty::test]
async fn several_periodic_sources_can_share_one_actor(cx: TaskContext) {
    let clock_control = ClockControl::new();
    let clock = clock_control.clone().to_clock();
    let runtime = ActorRuntime::from_builtins(&cx);
    let (tx, rx) = mpsc::bounded(4);
    let actor = runtime.builder().spawn(SinkActor { tx });
    runtime.send_interval(&actor, &clock, Duration::from_secs(1), || TestMessage::FastTick);
    runtime.send_interval(&actor, &clock, Duration::from_secs(2), || TestMessage::SlowTick);
    let actor: ActorRef<TestMessage> = actor.into();
    prime_actor(&cx, &actor, &rx).await;

    clock_control.advance(Duration::from_secs(2));
    actor
        .tell(TestMessage::Mailbox)
        .await
        .expect("mailbox message should wake the actor");
    let delivered = receive_many(&cx, &rx, 3).await;
    assert!(delivered.contains(&TestMessage::FastTick));
    assert!(delivered.contains(&TestMessage::SlowTick));
    assert!(delivered.contains(&TestMessage::Mailbox));

    actor.stop().await.expect("actor should stop cleanly");
}

#[arty::test]
async fn periodic_source_can_share_an_actor_with_an_external_stream(cx: TaskContext) {
    let clock_control = ClockControl::new();
    let clock = clock_control.clone().to_clock();
    let runtime = ActorRuntime::from_builtins(&cx);
    let (tx, rx) = mpsc::bounded(4);
    let actor = runtime
        .builder()
        .with_stream(stream::iter([TestMessage::Stream]))
        .spawn(SinkActor { tx });
    runtime.send_interval(&actor, &clock, Duration::from_secs(1), || TestMessage::FastTick);
    let actor: ActorRef<TestMessage> = actor.into();

    assert_eq!(yield_until_received(&cx, &rx).await, TestMessage::Stream);
    prime_actor(&cx, &actor, &rx).await;
    clock_control.advance(Duration::from_secs(1));
    actor
        .tell(TestMessage::Mailbox)
        .await
        .expect("mailbox message should wake the actor");
    let delivered = receive_many(&cx, &rx, 2).await;
    assert!(delivered.contains(&TestMessage::FastTick));
    assert!(delivered.contains(&TestMessage::Mailbox));

    actor.stop().await.expect("actor should stop cleanly");
}

#[arty::test]
async fn stopping_actor_cancels_periodic_delivery(cx: TaskContext) {
    let clock_control = ClockControl::new();
    let clock = clock_control.clone().to_clock();
    let runtime = ActorRuntime::from_builtins(&cx);
    let (tx, rx) = mpsc::bounded(4);
    let actor = runtime.builder().spawn(SinkActor { tx });
    runtime.send_interval(&actor, &clock, Duration::from_secs(1), || TestMessage::FastTick);
    let actor: ActorRef<TestMessage> = actor.into();

    actor.stop().await.expect("actor should stop cleanly");
    clock_control.advance(Duration::from_secs(1));
    yield_runtime(&cx).await;

    assert!(rx.try_recv().is_err(), "stopped actor should not run timer work");
}
