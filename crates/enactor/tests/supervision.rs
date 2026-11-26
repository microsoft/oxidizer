// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for exit notices sent to a supervisor.

use arty::task::Context as TaskContext;
use enactor::{Actor, ActorExit, ActorRef, ActorRuntime, Context};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Notice {
    monitor: u8,
    exit: ActorExit,
}

struct Recorder {
    notices: flume::Sender<Notice>,
}

impl Actor for Recorder {
    type Message = Notice;

    async fn handle(&mut self, notice: Notice, _ctx: &mut Context<Notice>) {
        self.notices
            .send_async(notice)
            .await
            .expect("the test keeps the notice receiver open");
    }
}

fn spawn_recorder(runtime: &ActorRuntime) -> (ActorRef<Notice>, flume::Receiver<Notice>) {
    let (notices, received) = flume::unbounded();
    (runtime.spawn(Recorder { notices }), received)
}

enum Command {
    Panic,
}

struct Worker;

impl Actor for Worker {
    type Message = Command;

    async fn handle(&mut self, command: Command, _ctx: &mut Context<Command>) {
        match command {
            #[expect(clippy::panic, reason = "the test needs a worker that panics")]
            Command::Panic => panic!("supervised worker failure"),
        }
    }
}

/// A supervisor message that reports when it is dropped.
struct Tracked {
    _dropped: oneshot::Sender<()>,
}

struct Sink;

impl Actor for Sink {
    type Message = Tracked;

    async fn handle(&mut self, _tracked: Tracked, _ctx: &mut Context<Tracked>) {}
}

#[arty::test]
async fn stop_sends_a_stopped_notice(cx: TaskContext) {
    let runtime = ActorRuntime::from_builtins(&cx);
    let (supervisor, notices) = spawn_recorder(&runtime);
    let worker = runtime.builder().spawn(Worker);
    runtime.monitor(&worker, &supervisor, |exit| Notice { monitor: 1, exit });

    worker.actor().stop().await.unwrap();

    assert_eq!(
        notices.recv_async().await.unwrap(),
        Notice {
            monitor: 1,
            exit: ActorExit::Stopped
        }
    );
}

#[arty::test]
async fn drain_sends_a_drained_notice(cx: TaskContext) {
    let runtime = ActorRuntime::from_builtins(&cx);
    let (supervisor, notices) = spawn_recorder(&runtime);
    let worker = runtime.builder().spawn(Worker);
    runtime.monitor(&worker, &supervisor, |exit| Notice { monitor: 1, exit });

    worker.actor().drain().await.unwrap();

    assert_eq!(
        notices.recv_async().await.unwrap(),
        Notice {
            monitor: 1,
            exit: ActorExit::Drained
        }
    );
}

#[arty::test(panic = ignore)]
async fn panic_sends_a_panicked_notice(cx: TaskContext) {
    let runtime = ActorRuntime::from_builtins(&cx);
    let (supervisor, notices) = spawn_recorder(&runtime);
    let worker = runtime.builder().spawn(Worker);
    runtime.monitor(&worker, &supervisor, |exit| Notice { monitor: 1, exit });

    worker.actor().tell(Command::Panic).await.unwrap();

    assert_eq!(
        notices.recv_async().await.unwrap(),
        Notice {
            monitor: 1,
            exit: ActorExit::Panicked
        }
    );
}

#[arty::test]
async fn monitor_after_exit_still_sends_a_notice(cx: TaskContext) {
    let runtime = ActorRuntime::from_builtins(&cx);
    let (supervisor, notices) = spawn_recorder(&runtime);
    let worker = runtime.builder().spawn(Worker);
    worker.actor().stop().await.unwrap();
    assert_eq!(worker.exit().await, ActorExit::Stopped);

    runtime.monitor(&worker, &supervisor, |exit| Notice { monitor: 1, exit });

    assert_eq!(
        notices.recv_async().await.unwrap(),
        Notice {
            monitor: 1,
            exit: ActorExit::Stopped
        }
    );
}

#[arty::test]
async fn two_monitors_each_send_one_notice(cx: TaskContext) {
    let runtime = ActorRuntime::from_builtins(&cx);
    let (supervisor, notices) = spawn_recorder(&runtime);
    let worker = runtime.builder().spawn(Worker);
    runtime.monitor(&worker, &supervisor, |exit| Notice { monitor: 1, exit });
    runtime.monitor(&worker, &supervisor, |exit| Notice { monitor: 2, exit });

    worker.actor().drain().await.unwrap();
    let mut received = vec![notices.recv_async().await.unwrap(), notices.recv_async().await.unwrap()];
    received.sort_by_key(|notice| notice.monitor);

    assert_eq!(
        received,
        [
            Notice {
                monitor: 1,
                exit: ActorExit::Drained
            },
            Notice {
                monitor: 2,
                exit: ActorExit::Drained
            },
        ]
    );
}

#[arty::test]
async fn closed_supervisor_drops_the_notice(cx: TaskContext) {
    let runtime = ActorRuntime::from_builtins(&cx);
    let supervisor = runtime.builder().spawn(Sink);
    supervisor.actor().stop().await.unwrap();
    assert_eq!(supervisor.exit().await, ActorExit::Stopped);
    let worker = runtime.builder().spawn(Worker);
    let (dropped, notice_dropped) = oneshot::channel();
    runtime.monitor(&worker, supervisor.actor(), move |_exit| Tracked { _dropped: dropped });

    worker.actor().stop().await.unwrap();

    notice_dropped.await.unwrap_err();
}
