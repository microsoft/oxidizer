// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Router actor forwarding work across two workers.

use enactor::{Actor, ActorRef, ActorRuntime, Context, Reply};

enum WorkerMessage {
    Describe(u32, Reply<String>),
}

struct WorkerActor {
    id: u8,
}

impl Actor for WorkerActor {
    type Message = WorkerMessage;

    async fn handle(&mut self, msg: WorkerMessage, _ctx: &mut Context<WorkerMessage>) {
        match msg {
            WorkerMessage::Describe(value, reply) => {
                let _ = reply.send(format!("worker-{} handled {value}", self.id));
            }
        }
    }
}

enum RouterMessage {
    Dispatch(u32, Reply<String>),
}

struct RouterActor {
    workers: Vec<ActorRef<WorkerMessage>>,
    next: usize,
}

impl Actor for RouterActor {
    type Message = RouterMessage;

    async fn handle(&mut self, msg: RouterMessage, _ctx: &mut Context<RouterMessage>) {
        match msg {
            RouterMessage::Dispatch(value, reply) => {
                let worker = self.workers[self.next % self.workers.len()].clone();
                self.next += 1;

                let response = enactor::ask!(worker, WorkerMessage::Describe(value))
                    .await
                    .expect("worker ask failed");

                let _ = reply.send(response);
            }
        }
    }
}

#[arty::main]
async fn main(cx: arty::task::Context) {
    let rt = ActorRuntime::from_builtins(&cx);

    let worker_a: ActorRef<WorkerMessage> = rt.spawn(WorkerActor { id: 1 });
    let worker_b: ActorRef<WorkerMessage> = rt.spawn(WorkerActor { id: 2 });
    let router: ActorRef<RouterMessage> = rt.spawn(RouterActor {
        workers: vec![worker_a.clone(), worker_b.clone()],
        next: 0,
    });

    let first: String = enactor::ask!(router, RouterMessage::Dispatch(3))
        .await
        .expect("first dispatch failed");
    let second: String = enactor::ask!(router, RouterMessage::Dispatch(5))
        .await
        .expect("second dispatch failed");
    let third: String = enactor::ask!(router, RouterMessage::Dispatch(8))
        .await
        .expect("third dispatch failed");

    assert_eq!(first, "worker-1 handled 3");
    assert_eq!(second, "worker-2 handled 5");
    assert_eq!(third, "worker-1 handled 8");

    router.stop().await.expect("router stop failed");
    worker_a.stop().await.expect("worker A stop failed");
    worker_b.stop().await.expect("worker B stop failed");
}
