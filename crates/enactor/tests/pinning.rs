// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for actor pinning behavior.

use std::thread;

use arty::task::Context as TaskContext;
use enactor::{Actor, ActorRef, ActorRuntime, ActorWithPlacement, Context, Reply};

enum ThreadMessage {
    Ping(Reply<thread::ThreadId>),
}

struct ThreadActor {
    thread_id: Option<thread::ThreadId>,
}

impl Actor for ThreadActor {
    type Message = ThreadMessage;

    async fn handle(&mut self, msg: ThreadMessage, _ctx: &mut Context<ThreadMessage>) {
        match msg {
            ThreadMessage::Ping(reply) => {
                let current = thread::current().id();
                if let Some(previous) = self.thread_id {
                    debug_assert_eq!(previous, current);
                } else {
                    self.thread_id = Some(current);
                }
                let _ = reply.send(current);
            }
        }
    }
}

fn spawn_thread_actor(rt: &ActorRuntime) -> ActorWithPlacement<ThreadMessage> {
    rt.spawn_anywhere(ThreadActor { thread_id: None })
}

#[arty::test]
async fn actor_is_pinned(cx: TaskContext) {
    let rt = ActorRuntime::from_builtins(&cx);
    let spawned = spawn_thread_actor(&rt);
    let actor = spawned.actor();

    let first = actor.ask(ThreadMessage::Ping).await.expect("first call failed");
    let second = actor.ask(ThreadMessage::Ping).await.expect("second call failed");

    assert_eq!(first, second);

    actor.stop().await.expect("stop failed");
}

#[arty::test(workers = 2)]
async fn actor_can_share_thread_with_previously_spawned_actor(cx: TaskContext) {
    let rt = ActorRuntime::from_builtins(&cx);
    let first = spawn_thread_actor(&rt);
    let second = rt
        .builder()
        .with_meta(enactor::Placement::SameThreadAs(first.placement()))
        .spawn(ThreadActor { thread_id: None });

    let first_thread = first.actor().ask(ThreadMessage::Ping).await.expect("first actor call failed");
    let second_thread = second.actor().ask(ThreadMessage::Ping).await.expect("second actor call failed");

    assert_eq!(first_thread, second_thread);

    first.actor().stop().await.expect("first actor stop failed");
    second.actor().stop().await.expect("second actor stop failed");
}

#[arty::test]
async fn spawn_return_types_are_consistent(cx: TaskContext) {
    let rt = ActorRuntime::from_builtins(&cx);
    let actor_ref: ActorRef<ThreadMessage> = rt.spawn(ThreadActor { thread_id: None });
    let spawned_anywhere: ActorWithPlacement<ThreadMessage> = rt.spawn_anywhere(ThreadActor { thread_id: None });
    let spawned_from_builder: ActorWithPlacement<ThreadMessage> = rt.builder().spawn(ThreadActor { thread_id: None });
    let builder_actor_ref: ActorRef<ThreadMessage> = spawned_from_builder.into();

    actor_ref.stop().await.expect("actor stop failed");
    spawned_anywhere.actor().stop().await.expect("anywhere actor stop failed");
    builder_actor_ref.stop().await.expect("builder actor stop failed");
}
