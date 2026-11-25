// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Basic actor runtime tests.

use enactor::{Actor, ActorRef, ActorRuntime, Context, Reply};

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
