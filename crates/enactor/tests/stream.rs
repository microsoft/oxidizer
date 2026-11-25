// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use arty::task::Context as TaskContext;
use enactor::{Actor, ActorRef, ActorRuntime, Context};
use flume as mpsc;
use futures::stream;

#[derive(Debug)]
struct SinkActor {
    tx: mpsc::Sender<u32>,
}

impl Actor for SinkActor {
    type Message = u32;

    async fn handle(&mut self, message: Self::Message, _ctx: &mut Context<Self::Message>) {
        self.tx
            .send_async(message)
            .await
            .expect("sink receiver should stay open during the test");
    }
}

#[arty::test]
async fn with_stream_merges_channel_and_stream(cx: TaskContext) {
    let runtime = ActorRuntime::from_builtins(&cx);
    let (tx, rx) = mpsc::bounded(8);
    let actor: ActorRef<u32> = runtime
        .builder()
        .with_stream(stream::iter([10_u32, 20, 30]))
        .spawn(SinkActor { tx })
        .into();

    actor.tell(1).await.expect("direct message should send");

    // One channel message plus three stream items reach the same handler.
    let mut received = Vec::new();
    for _ in 0..4 {
        received.push(rx.recv_async().await.expect("item should arrive"));
    }
    received.sort_unstable();
    assert_eq!(received, [1, 10, 20, 30]);

    // The actor still receives messages after the stream is exhausted.
    actor.tell(2).await.expect("actor should remain addressable after the stream ends");
    assert_eq!(rx.recv_async().await.expect("post-stream message should arrive"), 2);

    actor.stop().await.expect("merged actor should stop cleanly");
}

#[arty::test]
async fn with_stream_delivers_all_stream_items(cx: TaskContext) {
    let runtime = ActorRuntime::from_builtins(&cx);
    let (tx, rx) = mpsc::bounded(8);
    let actor: ActorRef<u32> = runtime.builder().with_stream(stream::iter(0_u32..5)).spawn(SinkActor { tx }).into();

    // With no channel traffic, every stream item still reaches the handler.
    let mut received = Vec::new();
    for _ in 0..5 {
        received.push(rx.recv_async().await.expect("stream item should arrive"));
    }
    received.sort_unstable();
    assert_eq!(received, [0, 1, 2, 3, 4]);

    actor.stop().await.expect("stream actor should stop cleanly");
}

#[arty::test]
async fn empty_stream_actor(cx: TaskContext) {
    let runtime = ActorRuntime::from_builtins(&cx);
    let (tx, rx) = mpsc::bounded(8);
    let actor: ActorRef<u32> = runtime.builder().with_stream(stream::empty::<u32>()).spawn(SinkActor { tx }).into();

    // The stream ends immediately; the actor still serves its mailbox.
    actor.tell(7).await.expect("channel message should send");
    assert_eq!(rx.recv_async().await.expect("message should arrive"), 7);

    actor.stop().await.expect("actor should stop cleanly");
}
