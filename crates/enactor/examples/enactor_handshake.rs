// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A two-step protocol written as a receive loop.
//!
//! The actor waits for `Hello`, then for `Data`. No state enum records which
//! step comes next, because the position in the code is the state.

use enactor::{ActorRuntime, Context, Inbox, Reply};

enum Handshake {
    Hello(String),
    Data(u32, Reply<String>),
}

#[arty::main]
async fn main(cx: arty::task::Context) {
    let runtime = ActorRuntime::from_builtins(&cx);
    let spawned = runtime
        .builder()
        .spawn_with(async |inbox: &mut Inbox<Handshake>, _ctx: &Context<Handshake>| {
            let Ok(Handshake::Hello(name)) = inbox.recv().await else {
                return;
            };
            println!("step 1: hello from {name}");

            let Ok(Handshake::Data(value, reply)) = inbox.recv().await else {
                return;
            };
            println!("step 2: {name} sent {value}");
            let _ = reply.send(format!("{name} finished the handshake"));
        });

    let actor = spawned.actor();
    actor
        .tell(Handshake::Hello("Ada".to_string()))
        .await
        .expect("the actor accepts the first step");
    let done = enactor::ask!(actor, Handshake::Data(42))
        .await
        .expect("the actor answers the second step");
    println!("{done}");
    println!("exit: {}", spawned.exit().await);
}
