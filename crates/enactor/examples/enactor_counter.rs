// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tell-driven counter updates.

use enactor::{Actor, ActorRef, ActorRuntime, Context};

struct Increment {
    by: u64,
}

struct Snapshot {
    reply: oneshot::Sender<u64>,
}

enum CounterMessage {
    Increment(Increment),
    Snapshot(Snapshot),
}

struct CounterActor {
    total: u64,
}

impl Actor for CounterActor {
    type Message = CounterMessage;

    async fn handle(&mut self, msg: CounterMessage, _ctx: &mut Context<CounterMessage>) {
        match msg {
            CounterMessage::Increment(Increment { by }) => {
                self.total += by;
            }
            CounterMessage::Snapshot(Snapshot { reply }) => {
                let _ = reply.send(self.total);
            }
        }
    }
}

#[arty::main]
async fn main(cx: arty::task::Context) {
    let rt = ActorRuntime::from_builtins(&cx);
    let counter: ActorRef<CounterMessage> = rt.spawn(CounterActor { total: 0 });

    counter
        .tell(CounterMessage::Increment(Increment { by: 2 }))
        .await
        .expect("first tell failed");
    enactor::tell!(counter, CounterMessage::Increment(Increment { by: 3 }))
        .await
        .expect("second tell failed");

    let (snapshot_tx, snapshot_rx) = oneshot::channel();
    counter
        .tell(CounterMessage::Snapshot(Snapshot { reply: snapshot_tx }))
        .await
        .expect("snapshot tell failed");

    let total = snapshot_rx.await.expect("snapshot should be received");
    assert_eq!(total, 5);

    counter.stop().await.expect("stop failed");
}
