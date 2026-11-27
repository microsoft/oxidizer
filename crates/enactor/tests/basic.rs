// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Basic actor runtime tests.

use std::collections::HashSet;

use enactor::{Actor, ActorId, ActorRef, ActorRuntime, Context, Reply};

struct Reset {
    ack: oneshot::Sender<()>,
}

enum Message {
    Ping(Reply<String>),
    Reset(Reset),
}

struct PongActor {
    count: u64,
}

impl Actor for PongActor {
    type Message = Message;

    async fn handle(&mut self, msg: Message, _ctx: &mut Context<Message>) {
        match msg {
            Message::Ping(reply) => {
                self.count += 1;
                let _ = reply.send(format!("pong #{}", self.count));
            }
            Message::Reset(message) => {
                self.count = 0;
                let _ = message.ack.send(());
            }
        }
    }
}

#[arty::test]
async fn basic_ping_pong(cx: arty::task::Context) {
    let rt = ActorRuntime::from_builtins(&cx);
    let pong: ActorRef<Message> = rt.spawn(PongActor { count: 0 });

    let reply: String = pong.ask(Message::Ping).await.expect("ping failed");
    assert_eq!(reply, "pong #1");

    let (reset_tx, reset_rx) = oneshot::channel();
    enactor::tell!(pong, Message::Reset(Reset { ack: reset_tx }))
        .await
        .expect("reset tell failed");
    reset_rx.await.expect("reset should be processed");

    let reply: String = pong.ask(Message::Ping).await.expect("second ping failed");
    assert_eq!(reply, "pong #1");

    pong.stop().await.expect("stop failed");
}

enum Identify {
    Id(Reply<ActorId>),
}

struct IdActor;

impl Actor for IdActor {
    type Message = Identify;

    async fn handle(&mut self, msg: Identify, ctx: &mut Context<Identify>) {
        let Identify::Id(reply) = msg;
        let _ = reply.send(ctx.id());
    }
}

#[arty::test]
async fn clones_of_a_handle_are_equal(cx: arty::task::Context) {
    let actor: ActorRef<Identify> = ActorRuntime::from_builtins(&cx).spawn(IdActor);

    let clone = actor.clone();

    assert!(clone == actor);
}

#[arty::test]
async fn spawns_from_different_runtimes_get_different_ids(cx: arty::task::Context) {
    let first: ActorRef<Identify> = ActorRuntime::from_builtins(&cx).spawn(IdActor);
    let second: ActorRef<Identify> = ActorRuntime::from_builtins(&cx).spawn(IdActor);

    assert_ne!(first.id(), second.id());
}

#[arty::test]
async fn hash_set_keeps_one_handle_per_actor(cx: arty::task::Context) {
    let runtime = ActorRuntime::from_builtins(&cx);
    let first: ActorRef<Identify> = runtime.spawn(IdActor);
    let second: ActorRef<Identify> = runtime.spawn(IdActor);

    #[expect(
        clippy::mutable_key_type,
        reason = "ActorRef compares and hashes only its ActorId, which never changes"
    )]
    let handles: HashSet<ActorRef<Identify>> = [first.clone(), second.clone(), first, second].into_iter().collect();

    assert_eq!(handles.len(), 2);
}

#[arty::test]
async fn context_id_matches_the_spawned_id(cx: arty::task::Context) {
    let spawned = ActorRuntime::from_builtins(&cx).builder().spawn(IdActor);

    let id = spawned.actor().ask(Identify::Id).await.unwrap();

    assert_eq!(id, spawned.id());
}
