// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Ask round trip with a single actor.

use enactor::{Actor, ActorRef, ActorRuntime, Context, Reply};

enum Message {
    Ping(Reply<String>),
}

struct PongActor;

impl Actor for PongActor {
    type Message = Message;
    async fn handle(&mut self, msg: Message, _ctx: &mut Context<Message>) {
        match msg {
            Message::Ping(reply) => {
                let _ = reply.send("pong".to_string());
            }
        }
    }
}

#[arty::main]
async fn main(cx: arty::task::Context) {
    let rt = ActorRuntime::from_builtins(&cx);
    let pong: ActorRef<Message> = rt.spawn(PongActor);
    let reply: String = enactor::ask!(pong, Message::Ping).await.expect("ask failed");
    assert_eq!(reply, "pong");
    pong.drain().await.expect("drain failed");
}
