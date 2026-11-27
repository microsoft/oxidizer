// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for actors written as a receive loop.

use std::cell::RefCell;
use std::rc::Rc;

use arty::task::Context as TaskContext;
use enactor::{ActorExit, ActorRef, ActorRuntime, Context, Inbox, Reply};
use futures::poll;

struct Gate {
    entered: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
}

impl Gate {
    fn new() -> (Self, oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (entered, entered_rx) = oneshot::channel();
        let (release, release_rx) = oneshot::channel();
        (
            Self {
                entered,
                release: release_rx,
            },
            entered_rx,
            release,
        )
    }

    /// Reports that the body reached the gate, then waits until the test opens it.
    async fn pass(self) {
        let _ = self.entered.send(());
        let _ = self.release.await;
    }
}

enum Step {
    Hello(Gate),
    Data(u32, Reply<u32>),
}

enum Count {
    Add(u32),
    Total(Reply<u32>),
}

#[arty::test]
async fn body_runs_a_two_step_protocol(cx: TaskContext) {
    let spawned = ActorRuntime::from_builtins(&cx)
        .builder()
        .spawn_with(async |inbox: &mut Inbox<Step>, _ctx: &Context<Step>| {
            let Ok(Step::Hello(gate)) = inbox.recv().await else {
                return;
            };
            gate.pass().await;
            let Ok(Step::Data(value, reply)) = inbox.recv().await else {
                return;
            };
            let _ = reply.send(value + 1);
        });
    let (gate, entered, release) = Gate::new();
    spawned.actor().tell(Step::Hello(gate)).await.unwrap();
    entered.await.unwrap();
    release.send(()).unwrap();

    let reply = spawned.actor().ask_with(Step::Data, 41).await.unwrap();

    assert_eq!((reply, spawned.exit().await), (42, ActorExit::Stopped));
}

#[arty::test]
async fn stop_while_the_body_waits_for_step_two_reports_stopped(cx: TaskContext) {
    let spawned = ActorRuntime::from_builtins(&cx)
        .builder()
        .spawn_with(async |inbox: &mut Inbox<Step>, _ctx: &Context<Step>| {
            let Ok(Step::Hello(gate)) = inbox.recv().await else {
                return;
            };
            gate.pass().await;
            while let Ok(Step::Data(value, reply)) = inbox.recv().await {
                let _ = reply.send(value);
            }
        });
    let (gate, entered, release) = Gate::new();
    spawned.actor().tell(Step::Hello(gate)).await.unwrap();
    entered.await.unwrap();
    release.send(()).unwrap();

    spawned.actor().stop().await.unwrap();

    assert_eq!(spawned.exit().await, ActorExit::Stopped);
}

#[arty::test]
async fn drain_delivers_buffered_messages_to_the_body(cx: TaskContext) {
    let spawned = ActorRuntime::from_builtins(&cx)
        .builder()
        .spawn_with(async |inbox: &mut Inbox<Step>, _ctx: &Context<Step>| {
            let Ok(Step::Hello(gate)) = inbox.recv().await else {
                return;
            };
            gate.pass().await;
            while let Ok(Step::Data(value, reply)) = inbox.recv().await {
                let _ = reply.send(value);
            }
        });
    let (gate, entered, release) = Gate::new();
    spawned.actor().tell(Step::Hello(gate)).await.unwrap();
    entered.await.unwrap();
    let (first, first_reply) = Reply::channel();
    let (second, second_reply) = Reply::channel();
    spawned.actor().tell(Step::Data(1, first)).await.unwrap();
    spawned.actor().tell(Step::Data(2, second)).await.unwrap();

    spawned.actor().drain().await.unwrap();
    release.send(()).unwrap();

    assert_eq!(
        (first_reply.await.unwrap(), second_reply.await.unwrap(), spawned.exit().await),
        (1, 2, ActorExit::Drained)
    );
}

#[arty::test]
async fn body_keeps_worker_local_state_across_an_await(cx: TaskContext) {
    let actor: ActorRef<Count> = ActorRuntime::from_builtins(&cx).spawn_with(async |inbox: &mut Inbox<Count>, _ctx: &Context<Count>| {
        // `Rc` is not `Send`, so this compiles only because the body's future may stay on its worker.
        let total = Rc::new(RefCell::new(0));
        while let Ok(message) = inbox.recv().await {
            match message {
                Count::Add(value) => *total.borrow_mut() += value,
                Count::Total(reply) => {
                    let _ = reply.send(*total.borrow());
                }
            }
        }
    });
    actor.tell(Count::Add(2)).await.unwrap();
    actor.tell(Count::Add(3)).await.unwrap();

    let total = actor.ask(Count::Total).await.unwrap();

    assert_eq!(total, 5);
}

#[arty::test]
async fn body_receives_stream_items(cx: TaskContext) {
    let (sum, summed) = oneshot::channel();
    let spawned = ActorRuntime::from_builtins(&cx)
        .builder()
        .with_stream(futures::stream::iter([1_u32, 2, 3]))
        .spawn_with(async move |inbox: &mut Inbox<u32>, _ctx: &Context<u32>| {
            let mut total = 0;
            for _ in 0..3 {
                total += inbox.recv().await.unwrap();
            }
            let _ = sum.send(total);
        });

    assert_eq!((summed.await.unwrap(), spawned.exit().await), (6, ActorExit::Stopped));
}

#[arty::test]
async fn early_return_cancels_a_buffered_ask(cx: TaskContext) {
    let spawned = ActorRuntime::from_builtins(&cx)
        .builder()
        .spawn_with(async |inbox: &mut Inbox<Step>, _ctx: &Context<Step>| {
            if let Ok(Step::Hello(gate)) = inbox.recv().await {
                gate.pass().await;
            }
        });
    let (gate, entered, release) = Gate::new();
    spawned.actor().tell(Step::Hello(gate)).await.unwrap();
    entered.await.unwrap();
    let mut ask = Box::pin(spawned.actor().ask_with(Step::Data, 1));
    assert!(poll!(&mut ask).is_pending());

    release.send(()).unwrap();

    assert!(ask.await.unwrap_err().is_canceled());
}

#[arty::test]
async fn early_return_during_a_drain_reports_stopped(cx: TaskContext) {
    let spawned = ActorRuntime::from_builtins(&cx)
        .builder()
        .spawn_with(async |inbox: &mut Inbox<Step>, _ctx: &Context<Step>| {
            if let Ok(Step::Hello(gate)) = inbox.recv().await {
                gate.pass().await;
            }
            // Start the drain by reading one buffered message, then leave the other unread.
            drop(inbox.try_recv());
        });
    let (gate, entered, release) = Gate::new();
    spawned.actor().tell(Step::Hello(gate)).await.unwrap();
    entered.await.unwrap();
    let (first, _first_reply) = Reply::channel();
    let (second, _second_reply) = Reply::channel();
    spawned.actor().tell(Step::Data(1, first)).await.unwrap();
    spawned.actor().tell(Step::Data(2, second)).await.unwrap();
    spawned.actor().drain().await.unwrap();

    release.send(()).unwrap();

    assert_eq!(spawned.exit().await, ActorExit::Stopped);
}
