// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A supervisor that restarts a worker after the worker panics.
//!
//! The supervisor asks the runtime to monitor its worker. When the worker
//! panics, the supervisor starts a new worker, monitors it, and replaces its
//! reference to the old worker.

use enactor::{Actor, ActorExit, ActorRef, ActorRuntime, Context, Reply};

const RESTARTS: u32 = 3;

enum WorkerMessage {
    Square(u64, Reply<u64>),
    Poison,
}

struct Worker;

impl Actor for Worker {
    type Message = WorkerMessage;

    async fn handle(&mut self, msg: WorkerMessage, _ctx: &mut Context<WorkerMessage>) {
        match msg {
            WorkerMessage::Square(value, reply) => {
                let _ = reply.send(value * value);
            }
            #[expect(clippy::panic, reason = "the example worker fails on purpose")]
            WorkerMessage::Poison => panic!("the worker received a poison message"),
        }
    }
}

enum SupervisorMessage {
    Forward(WorkerMessage),
    WorkerExited(ActorExit),
}

struct Supervisor {
    runtime: ActorRuntime,
    worker: ActorRef<WorkerMessage>,
    restarts: u32,
    restarted: flume::Sender<u32>,
}

impl Actor for Supervisor {
    type Message = SupervisorMessage;

    async fn handle(&mut self, msg: SupervisorMessage, ctx: &mut Context<SupervisorMessage>) {
        match msg {
            SupervisorMessage::Forward(message) => {
                self.worker.tell(message).await.expect("main forwards only to a running worker");
            }
            SupervisorMessage::WorkerExited(ActorExit::Panicked) => {
                let worker = self.runtime.builder().spawn(Worker);
                self.runtime.monitor(&worker, ctx.actor_ref(), SupervisorMessage::WorkerExited);
                self.worker = worker.into();
                self.restarts += 1;
                println!(
                    "restart {}: the worker panicked, so the supervisor started a new one",
                    self.restarts
                );
                self.restarted.send(self.restarts).expect("main waits for every restart");
            }
            SupervisorMessage::WorkerExited(_) => {}
        }
    }
}

#[arty::main(panic = ignore)]
async fn main(cx: arty::task::Context) {
    let runtime = ActorRuntime::from_builtins(&cx);
    let (restarted, restart_count) = flume::unbounded();
    let worker = runtime.builder().spawn(Worker);
    let supervisor: ActorRef<SupervisorMessage> = runtime.spawn(Supervisor {
        runtime: runtime.clone(),
        worker: worker.actor().clone(),
        restarts: 0,
        restarted,
    });
    runtime.monitor(&worker, &supervisor, SupervisorMessage::WorkerExited);

    for expected in 1..=RESTARTS {
        supervisor
            .tell(SupervisorMessage::Forward(WorkerMessage::Poison))
            .await
            .expect("the supervisor accepts work");
        let restarts = restart_count.recv_async().await.expect("the supervisor reports each restart");
        assert_eq!(restarts, expected);
    }

    let (reply, square) = Reply::channel();
    supervisor
        .tell(SupervisorMessage::Forward(WorkerMessage::Square(7, reply)))
        .await
        .expect("the supervisor accepts work");
    assert_eq!(square.await.expect("the newest worker answers"), 49);

    supervisor.stop().await.expect("supervisor stop failed");
}
