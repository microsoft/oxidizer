// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for asks with a deadline.

use std::error::Error;
use std::time::Duration;

use arty::task::Context as TaskContext;
use enactor::{Actor, ActorRef, ActorRuntime, AskError, Context, Reply, TellError};
use tick::{Clock, ClockControl};

const TIMEOUT: Duration = Duration::from_secs(1);

struct Gate {
    started: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
}

enum Request {
    Echo(u32, Reply<u32>),
    Ping(Reply<()>),
    Hold(Reply<u32>),
    Block(Gate),
}

#[derive(Default)]
struct Responder {
    held: Vec<Reply<u32>>,
}

impl Actor for Responder {
    type Message = Request;

    async fn handle(&mut self, request: Request, _ctx: &mut Context<Request>) {
        match request {
            Request::Echo(value, reply) => {
                let _ = reply.send(value);
            }
            Request::Ping(reply) => {
                let _ = reply.send(());
            }
            Request::Hold(reply) => self.held.push(reply),
            Request::Block(gate) => {
                let _ = gate.started.send(());
                let _ = gate.release.await;
            }
        }
    }
}

/// A clock that never moves, so a deadline never passes.
fn frozen_clock() -> Clock {
    ClockControl::new().to_clock()
}

/// A clock that jumps to each timer deadline, so a deadline passes as soon as it is set.
fn rushing_clock() -> Clock {
    ClockControl::new().auto_advance_timers(true).to_clock()
}

fn is_tell_error(error: &AskError) -> bool {
    error.source().and_then(|source| source.downcast_ref::<TellError>()).is_some()
}

async fn block(actor: &ActorRef<Request>) -> oneshot::Sender<()> {
    let (started, started_rx) = oneshot::channel();
    let (release, release_rx) = oneshot::channel();
    actor
        .tell(Request::Block(Gate {
            started,
            release: release_rx,
        }))
        .await
        .expect("the actor accepts the blocking request");
    started_rx.await.expect("the actor starts the blocking request");
    release
}

#[arty::test]
async fn reply_before_the_deadline_is_returned(cx: TaskContext) {
    let actor: ActorRef<Request> = ActorRuntime::from_builtins(&cx).spawn(Responder::default());

    let value = actor.ask_with_timeout(Request::Echo, 7, &frozen_clock(), TIMEOUT).await.unwrap();

    assert_eq!(value, 7);
}

#[arty::test]
async fn missing_reply_times_out(cx: TaskContext) {
    let actor: ActorRef<Request> = ActorRuntime::from_builtins(&cx).spawn(Responder::default());

    let error = actor.ask_timeout(Request::Hold, &rushing_clock(), TIMEOUT).await.unwrap_err();

    assert_eq!(
        (error.is_timeout(), error.is_canceled(), is_tell_error(&error)),
        (true, false, false)
    );
}

#[arty::test]
async fn full_mailbox_past_the_deadline_times_out(cx: TaskContext) {
    let actor: ActorRef<Request> = ActorRuntime::from_builtins(&cx)
        .builder()
        .with_capacity(1)
        .spawn(Responder::default())
        .into();
    let release = block(&actor).await;
    let (queued, _queued_rx) = Reply::channel();
    actor.try_tell(Request::Echo(0, queued)).map_err(|_full| ()).unwrap();

    let error = actor
        .ask_with_timeout(Request::Echo, 1, &rushing_clock(), TIMEOUT)
        .await
        .unwrap_err();

    assert!(error.is_timeout());
    release.send(()).unwrap();
}

#[arty::test]
async fn stopped_actor_is_not_a_timeout(cx: TaskContext) {
    let actor: ActorRef<Request> = ActorRuntime::from_builtins(&cx).spawn(Responder::default());
    actor.stop().await.unwrap();

    let error = actor.ask_timeout(Request::Hold, &rushing_clock(), TIMEOUT).await.unwrap_err();

    assert_eq!((error.is_timeout(), is_tell_error(&error)), (false, true));
}

#[arty::test]
async fn ask_t_appends_the_reply_to_a_payload_variant(cx: TaskContext) {
    let actor: ActorRef<Request> = ActorRuntime::from_builtins(&cx).spawn(Responder::default());
    let clock = frozen_clock();

    let value = enactor::ask_t!(actor, Request::Echo(5), &clock, TIMEOUT).await.unwrap();

    assert_eq!(value, 5);
}

#[arty::test]
async fn ask_t_accepts_a_zero_payload_variant(cx: TaskContext) {
    let actor: ActorRef<Request> = ActorRuntime::from_builtins(&cx).spawn(Responder::default());
    let clock = frozen_clock();

    enactor::ask_t!(actor, Request::Ping, &clock, TIMEOUT).await.unwrap();
}
