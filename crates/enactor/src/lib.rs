// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Typed actors for the [Arty](arty) thread-per-core runtime.
//!
//! An actor keeps its own state and handles one message at a time. Each actor
//! runs as one Arty task, so it stays on one worker thread for its whole life.
//! Use [`ActorRuntime`] to spawn an actor, [`ActorRef`] to send it work, and
//! [`Context`] inside [`Actor::handle`] for self-sends and lifecycle requests.
//!
//! Each actor has two queues. Messages use a bounded mailbox. Stop and drain
//! requests use a separate control queue. The actor reads the control queue
//! before the next message, so a full mailbox does not delay a stop.
//!
//! Message types are usually enums. Tell-style variants carry only their
//! payload. Ask-style variants end with [`Reply<T>`](Reply), so the actor can
//! send one value back to the caller.
//!
//! # Getting started
//!
//! ```
//! use enactor::{Actor, ActorRef, ActorRuntime, Context, Reply};
//!
//! enum Message {
//!     Add(u64),
//!     Total(Reply<u64>),
//! }
//!
//! struct Counter {
//!     total: u64,
//! }
//!
//! impl Actor for Counter {
//!     type Message = Message;
//!
//!     async fn handle(&mut self, message: Message, _ctx: &mut Context<Message>) {
//!         match message {
//!             Message::Add(value) => self.total += value,
//!             Message::Total(reply) => {
//!                 let _ = reply.send(self.total);
//!             }
//!         }
//!     }
//! }
//!
//! #[arty::main]
//! async fn main(cx: arty::task::Context) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//!     let runtime = ActorRuntime::from_builtins(&cx);
//!     let counter: ActorRef<Message> = runtime.spawn(Counter { total: 0 });
//!
//!     counter.tell(Message::Add(2)).await?;
//!     enactor::tell!(counter, Message::Add(3)).await?;
//!     assert_eq!(counter.ask(Message::Total).await?, 5);
//!
//!     counter.stop().await?;
//!     Ok(())
//! }
//! ```
//!
//! Use [`ActorRuntime::spawn`] for current-thread placement,
//! [`ActorRuntime::spawn_anywhere`] for runtime-selected placement, or
//! [`ActorRuntime::builder`] for explicit placement, inbox capacity, or a
//! stream-backed actor.
//!
//! Send fire-and-forget work with [`ActorRef::tell`] or [`tell!`](macro@tell).
//! Use [`ActorRef::tell_owned`] or [`ActorRef::try_tell`] when a rejected
//! message must be returned to the caller.
//! Request a value back with [`ActorRef::ask`] for zero-payload ask variants.
//! [`ask!`](macro@ask) provides constructor-call syntax when the ask variant
//! also carries caller-provided payload.
//!
//! To limit how long an ask waits, use [`ActorRef::ask_timeout`],
//! [`ActorRef::ask_with_timeout`], or [`ask_t!`](macro@ask_t). One deadline
//! covers the wait for mailbox capacity and the wait for the reply. A request
//! that timed out can still be handled, because the mailbox may already own it.
//!
//! # Lifecycle and supervision
//!
//! [`ActorWithPlacement::exit`] reports how an actor ended as an
//! [`ActorExit`]: it stopped, it drained, it panicked, or the runtime aborted
//! it. [`ActorRuntime::monitor`] sends that exit to a supervisor actor as a
//! message, so the supervisor can start a new actor after a panic.
//!
//! Each actor has an [`ActorId`] that no other actor in the process has.
//! [`ActorRef`] handles compare and hash by that id.
//!
//! Inside [`Actor::handle`], [`Context::runtime`] spawns child actors. A child
//! spawned with no placement runs on the same worker as its parent.
//!
//! # Receive loops
//!
//! An actor can also be an async closure that reads its own [`Inbox`]. Spawn
//! it with [`ActorSpawner::spawn_with`] or [`ActorRuntime::spawn_with`]. A
//! protocol with several steps then reads as straight-line code, and the
//! closure can keep worker-local state, such as an `Rc`, across an `.await`.
//! The actor ends when the closure returns.
//!
//! The `examples` folder contains `enactor_ping_pong`, `enactor_counter`,
//! `enactor_worker_pool`, `enactor_supervisor`, and `enactor_handshake`.

extern crate self as enactor;

#[doc(hidden)]
pub mod actor;
mod error;
mod exit;
mod inbox;
mod macros;
mod message;
mod placement;
mod reply;
mod runtime;
mod spawn;

pub use placement::{Placement, PlacementToken, TaskMeta};

#[doc(inline)]
pub use actor::{Actor, ActorId, ActorRef, Context, TryTellFailure};
pub use error::{AskError, CompletionTimeoutError, TellError};
pub use exit::ActorExit;
pub use inbox::Inbox;
pub use spawn::{ActorSpawner, ActorWithPlacement, NoStream, WithStream};

#[doc(inline)]
pub use message::Message;
#[doc(inline)]
pub use reply::Reply;
pub use runtime::ActorRuntime;
