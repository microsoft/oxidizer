// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for actor context behavior.

use arty::task::Context as TaskContext;
use enactor::{Actor, ActorRef, ActorRuntime, Context};

struct Ack {
    ack: oneshot::Sender<()>,
}

struct SelfStop {
    ack: oneshot::Sender<()>,
}

enum ContextMessage {
    RequestStop(SelfStop),
    Ack(Ack),
}

struct ContextActor;

impl Actor for ContextActor {
    type Message = ContextMessage;

    async fn handle(&mut self, msg: Self::Message, ctx: &mut Context<Self::Message>) {
        match msg {
            ContextMessage::RequestStop(message) => {
                let _ = message.ack.send(());
                ctx.stop();
            }
            ContextMessage::Ack(message) => {
                let _ = message.ack.send(());
            }
        }
    }
}

struct Schedule(oneshot::Sender<()>);

struct Emit(oneshot::Sender<()>);

enum ContextHandleMessage {
    Schedule(Schedule),
    Emit(Emit),
}

struct ContextHandleActor;

impl Actor for ContextHandleActor {
    type Message = ContextHandleMessage;

    async fn handle(&mut self, msg: Self::Message, ctx: &mut Context<Self::Message>) {
        match msg {
            ContextHandleMessage::Schedule(Schedule(ack)) => {
                ctx.actor_ref()
                    .tell(ContextHandleMessage::Emit(Emit(ack)))
                    .await
                    .expect("self send should succeed");
            }
            ContextHandleMessage::Emit(Emit(ack)) => {
                let _ = ack.send(());
            }
        }
    }
}

#[arty::test]
async fn actor_context_can_stop_actor(cx: TaskContext) {
    let rt = ActorRuntime::from_builtins(&cx);
    let actor: ActorRef<ContextMessage> = rt.spawn(ContextActor);

    let (stop_ack_tx, stop_ack_rx) = oneshot::channel();

    actor
        .tell(ContextMessage::RequestStop(SelfStop { ack: stop_ack_tx }))
        .await
        .expect("failed to queue stop message");

    stop_ack_rx.await.expect("actor should process the stop-requesting turn");

    let (late_tx, _late_rx) = oneshot::channel();
    actor
        .tell(ContextMessage::Ack(Ack { ack: late_tx }))
        .await
        .expect_err("actor should stop itself after the current turn");
}

#[arty::test]
async fn test_context_can_send_to_self(cx: TaskContext) {
    let rt = ActorRuntime::from_builtins(&cx);
    let actor: ActorRef<ContextHandleMessage> = rt.spawn(ContextHandleActor);
    let (ack_tx, ack_rx) = oneshot::channel();

    actor
        .tell(ContextHandleMessage::Schedule(Schedule(ack_tx)))
        .await
        .expect("failed to schedule self-send");

    ack_rx.await.expect("actor should process self-sent message");
    actor.stop().await.expect("stop failed");
}
