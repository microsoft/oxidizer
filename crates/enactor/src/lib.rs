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
//! The `examples` folder contains `enactor_ping_pong`, `enactor_counter`, and
//! `enactor_worker_pool`.

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
