// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Request-reply overhead of an actor, compared with a plain task.
//!
//! The workload follows the `overhead` benchmark of the kameo actor library.
//! Each iteration sends one `u32` and waits until the same value comes back.
//!
//! - `actor` uses [`ActorRef::ask_with`](enactor::ActorRef::ask_with).
//! - `bounded_ask_timeout` uses [`ActorRef::ask_with_timeout`](enactor::ActorRef::ask_with_timeout)
//!   with a deadline that never passes, so it shows the cost of the deadline.
//! - `bounded_ask_receive_loop` uses `ActorRef::ask_with` on an actor written
//!   as a receive loop with [`ActorSpawner::spawn_with`](enactor::ActorSpawner::spawn_with).
//! - `plain_task` uses a task that reads a bounded `flume` channel and replies
//!   through a `oneshot` channel. These are the same channels that the actor
//!   uses, so the difference between `actor` and `plain_task` is the cost of
//!   the actor layer.
//!
//! Every mailbox holds 10 messages, as in the kameo benchmark. Each sample
//! spawns its receiver and sends one request before measurement starts.

#![allow(clippy::unwrap_used, reason = "benchmark code")]

use std::time::Duration;

use arty::runtime::Runtime;
use arty::task::RuntimeScheduler;
use benchmarking::time_sample_async;
use criterion::{Criterion, criterion_group, criterion_main};
use enactor::{Actor, ActorRef, ActorRuntime, Context, Inbox, Reply};

const CAPACITY: usize = 10;

/// A deadline far longer than any sample, so no ask times out.
const ASK_TIMEOUT: Duration = Duration::from_mins(1);

enum Echo {
    Ask(u32, Reply<u32>),
}

struct EchoActor;

impl Actor for EchoActor {
    type Message = Echo;

    async fn handle(&mut self, msg: Echo, _ctx: &mut Context<Echo>) {
        let Echo::Ask(value, reply) = msg;
        let _ = reply.send(value);
    }
}

fn sample_actor(scheduler: &RuntimeScheduler, iters: u64) -> Duration {
    scheduler
        .block_on(async move |cx| {
            let actor: ActorRef<Echo> = ActorRuntime::from_builtins(&cx)
                .builder()
                .with_capacity(CAPACITY)
                .spawn(EchoActor)
                .into();
            actor.ask_with(Echo::Ask, 0).await.unwrap();

            let elapsed = time_sample_async(iters, |_| actor.ask_with(Echo::Ask, 0)).await;

            actor.stop().await.unwrap();
            elapsed
        })
        .unwrap()
}

fn sample_actor_timeout(scheduler: &RuntimeScheduler, iters: u64) -> Duration {
    scheduler
        .block_on(async move |cx| {
            let actor: ActorRef<Echo> = ActorRuntime::from_builtins(&cx)
                .builder()
                .with_capacity(CAPACITY)
                .spawn(EchoActor)
                .into();
            actor.ask_with_timeout(Echo::Ask, 0, cx.clock(), ASK_TIMEOUT).await.unwrap();

            let elapsed = time_sample_async(iters, |_| actor.ask_with_timeout(Echo::Ask, 0, cx.clock(), ASK_TIMEOUT)).await;

            actor.stop().await.unwrap();
            elapsed
        })
        .unwrap()
}

fn sample_receive_loop(scheduler: &RuntimeScheduler, iters: u64) -> Duration {
    scheduler
        .block_on(async move |cx| {
            let actor: ActorRef<Echo> = ActorRuntime::from_builtins(&cx)
                .builder()
                .with_capacity(CAPACITY)
                .spawn_with(async |inbox: &mut Inbox<Echo>, _ctx: &Context<Echo>| {
                    while let Ok(Echo::Ask(value, reply)) = inbox.recv().await {
                        let _ = reply.send(value);
                    }
                })
                .into();
            actor.ask_with(Echo::Ask, 0).await.unwrap();

            let elapsed = time_sample_async(iters, |_| actor.ask_with(Echo::Ask, 0)).await;

            actor.stop().await.unwrap();
            elapsed
        })
        .unwrap()
}

fn sample_plain_task(scheduler: &RuntimeScheduler, iters: u64) -> Duration {
    scheduler
        .block_on(async move |cx| {
            let (requests, inbox) = flume::bounded::<(u32, oneshot::Sender<u32>)>(CAPACITY);
            let _task = cx.scheduler().spawn(move |_| async move {
                while let Ok((value, reply)) = inbox.recv_async().await {
                    let _ = reply.send(value);
                }
            });
            let ask = async |value: u32| {
                let (reply, response) = oneshot::channel();
                requests.send_async((value, reply)).await.unwrap();
                response.await.unwrap()
            };
            ask(0).await;

            time_sample_async(iters, |_| ask(0)).await
        })
        .unwrap()
}

fn overhead(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();
    let scheduler = runtime.scheduler();

    let mut group = c.benchmark_group("enactor_overhead/bounded_ask");
    group.bench_function("actor", |b| b.iter_custom(|iters| sample_actor(scheduler, iters)));
    group.bench_function("bounded_ask_timeout", |b| {
        b.iter_custom(|iters| sample_actor_timeout(scheduler, iters));
    });
    group.bench_function("bounded_ask_receive_loop", |b| {
        b.iter_custom(|iters| sample_receive_loop(scheduler, iters));
    });
    group.bench_function("plain_task", |b| b.iter_custom(|iters| sample_plain_task(scheduler, iters)));
    group.finish();

    runtime.stop().unwrap();
}

criterion_group!(benches, overhead);
criterion_main!(benches);
