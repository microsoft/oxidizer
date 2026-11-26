// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Builder for actor spawn options.

use std::any::Any;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use arty::task::{JoinError, JoinHandle};
use futures::future::{BoxFuture, Either, Shared, select};
use futures::{FutureExt, Stream};
use thread_aware::Unaware;
use tick::Clock;

use crate::exit::LoopExit;
use crate::inbox::MessageStream;
use crate::runtime::{ActorRuntime, DEFAULT_CAPACITY, Dispatched};
use crate::{Actor, ActorExit, ActorId, ActorRef, CompletionTimeoutError, Context, Inbox, Message, PlacementToken, TaskMeta};

/// Completion of an actor task, shared by every clone of its handle.
type Completion = Shared<BoxFuture<'static, Outcome>>;

/// Converts an Arty join handle into a completion that every waiter can observe.
fn completion(task: JoinHandle<Unaware<LoopExit>>) -> Completion {
    task.map(Outcome::from_join).boxed().shared()
}

/// How an actor task ended, as seen by every waiter.
#[derive(Debug, Clone)]
enum Outcome {
    /// The actor loop returned.
    Returned(LoopExit),
    /// The actor task panicked.
    Panicked(PanicPayload),
    /// The runtime canceled or rejected the actor task.
    Aborted,
}

impl Outcome {
    fn from_join(result: Result<Unaware<LoopExit>, JoinError>) -> Self {
        match result {
            Ok(exit) => Self::Returned(exit.into_inner()),
            Err(error) => match error.into_panic() {
                Some(payload) => Self::Panicked(PanicPayload::new(payload)),
                // Arty reports a task that shutdown canceled, or that it rejected, without a payload.
                None => Self::Aborted,
            },
        }
    }

    fn exit(&self) -> ActorExit {
        match self {
            Self::Returned(exit) => ActorExit::from(*exit),
            Self::Panicked(_) => ActorExit::Panicked,
            Self::Aborted => ActorExit::Aborted,
        }
    }
}

/// The payload of an actor panic, kept for the first waiter that resumes it.
#[derive(Debug, Clone)]
struct PanicPayload(Arc<Mutex<Option<Box<dyn Any + Send>>>>);

impl PanicPayload {
    fn new(payload: Box<dyn Any + Send>) -> Self {
        Self(Arc::new(Mutex::new(Some(payload))))
    }

    /// Resumes the actor panic in the first caller. Later callers get a new panic.
    #[expect(
        clippy::panic,
        reason = "wait_for_completion documents that later waiters panic after an actor panic"
    )]
    fn resume(&self) -> ! {
        let payload = self
            .0
            .lock()
            .expect("the lock is held only to take the payload, which cannot panic")
            .take();
        match payload {
            Some(payload) => std::panic::resume_unwind(payload),
            None => panic!("the actor panicked, and another waiter received the panic payload"),
        }
    }
}

/// Spawner state: no stream is attached.
#[derive(Debug)]
pub struct NoStream;

/// Spawner state: a stream is attached and will be merged into the inbox.
pub struct WithStream<M: Message>(MessageStream<M>);

impl<M: Message> fmt::Debug for WithStream<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WithStream").finish_non_exhaustive()
    }
}

/// An actor reference together with the worker-thread placement it was assigned.
#[derive(Debug)]
pub struct ActorWithPlacement<M: Message> {
    actor: ActorRef<M>,
    placement: PlacementToken,
    completion: Completion,
}

impl<M: Message> Clone for ActorWithPlacement<M> {
    fn clone(&self) -> Self {
        Self {
            actor: self.actor.clone(),
            placement: self.placement.clone(),
            completion: self.completion.clone(),
        }
    }
}

impl<M: Message> ActorWithPlacement<M> {
    fn new((actor, placement, task): Dispatched<M>) -> Self {
        Self {
            actor,
            placement,
            completion: completion(task),
        }
    }

    /// Reference to the spawned actor.
    #[must_use]
    pub const fn actor(&self) -> &ActorRef<M> {
        &self.actor
    }

    /// Returns the id of the spawned actor.
    #[must_use]
    pub const fn id(&self) -> ActorId {
        self.actor.id()
    }

    /// Worker-thread placement assigned to the actor.
    #[must_use]
    pub fn placement(&self) -> PlacementToken {
        self.placement.clone()
    }

    /// Waits up to `timeout` for actor task exit without requesting stop or drain.
    ///
    /// Any exit other than a panic returns `Ok(())`, including an abort by the
    /// runtime. Use [`exit`](Self::exit) to learn how the actor ended.
    ///
    /// Completion includes dropping actor state, but does not prove that
    /// external effects or detached child tasks have settled. Sender handles
    /// can retain buffered messages after a stop.
    /// Clones can wait independently with different timeouts. Canceling or timing
    /// out a wait does not stop the actor or discard its completion handle.
    /// The timeout starts when this future is polled. A completed task wins over
    /// an expired timeout, including a zero timeout. The executor and clock must
    /// remain active for the timeout to fire.
    ///
    /// # Errors
    ///
    /// Returns [`CompletionTimeoutError`] when the task has not exited within
    /// `timeout`. Keep the owner alive and treat shutdown as incomplete; a later
    /// call can still observe completion.
    ///
    /// # Panics
    ///
    /// Resumes an actor task panic in the first waiter, with the original panic
    /// payload. Later waiters panic with a message that says another waiter
    /// received the payload. [`exit`](Self::exit) never panics.
    pub async fn wait_for_completion(&self, clock: &Clock, timeout: Duration) -> Result<(), CompletionTimeoutError> {
        let completion = self.completion.clone();
        let deadline = clock.delay(timeout);
        futures::pin_mut!(completion, deadline);
        match select(completion, deadline).await {
            Either::Left((outcome, _)) => match outcome {
                Outcome::Returned(_) | Outcome::Aborted => Ok(()),
                Outcome::Panicked(payload) => payload.resume(),
            },
            Either::Right(_) => Err(CompletionTimeoutError::new(timeout)),
        }
    }

    /// Waits for the actor task to end and reports how it ended.
    ///
    /// This future never panics, not even after an actor panic, and it has no
    /// timeout. It reports a panic as [`ActorExit::Panicked`] and leaves the
    /// panic payload for [`wait_for_completion`](Self::wait_for_completion).
    /// Every clone and every later call reports the same value. Waiting does not
    /// stop the actor.
    pub fn exit(&self) -> impl Future<Output = ActorExit> + Send + 'static {
        self.completion.clone().map(|outcome| outcome.exit())
    }
}

/// Consumes the spawn, discarding the placement, when only the actor is needed.
impl<M: Message> From<ActorWithPlacement<M>> for ActorRef<M> {
    fn from(spawned: ActorWithPlacement<M>) -> Self {
        spawned.actor
    }
}

/// Builder for actor placement, inbox capacity, and an optional merged stream.
///
// The `S` type parameter tracks whether a stream has been attached, so
// [`with_stream`](Self::with_stream) is available only before one is set.
pub struct ActorSpawner<'rt, S = NoStream> {
    runtime: &'rt ActorRuntime,
    task_meta: Option<TaskMeta>,
    capacity: usize,
    stream: S,
}

impl<S> fmt::Debug for ActorSpawner<'_, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ActorSpawner")
            .field("capacity", &self.capacity)
            .finish_non_exhaustive()
    }
}

impl<'rt> ActorSpawner<'rt, NoStream> {
    /// Initializes the builder.
    pub(crate) const fn new(runtime: &'rt ActorRuntime) -> Self {
        Self {
            runtime,
            task_meta: None,
            capacity: DEFAULT_CAPACITY,
            stream: NoStream,
        }
    }

    /// Configures `stream` as an additional source of messages.
    ///
    /// Stream items are delivered to [`Actor::handle`](crate::Actor::handle)
    /// exactly like channel messages. Buffered channel messages are prioritized before stream
    /// items.
    #[must_use]
    pub fn with_stream<M, St>(self, stream: St) -> ActorSpawner<'rt, WithStream<M>>
    where
        M: Message,
        St: Stream<Item = M> + Send + 'static,
    {
        ActorSpawner {
            runtime: self.runtime,
            task_meta: self.task_meta,
            capacity: self.capacity,
            stream: WithStream(Box::pin(stream)),
        }
    }

    /// Spawns the actor.
    pub fn spawn<A>(self, actor: A) -> ActorWithPlacement<A::Message>
    where
        A: Actor,
    {
        ActorWithPlacement::new(self.runtime.dispatch(self.task_meta, self.capacity, actor, None))
    }

    /// Spawns an actor written as a receive loop.
    ///
    /// The actor runs `body` once. The body gets the actor's [`Inbox`] and
    /// [`Context`], calls [`Inbox::recv`] for each message, and ends the actor
    /// when it returns. A stop or drain from an [`ActorRef`] still works,
    /// because `recv` returns an error after a stop, and after a drain delivers
    /// the last buffered message.
    ///
    /// The body gets `&Context`, not `&mut Context`. [`Context::stop`] and
    /// [`Context::drain`] take effect only between two calls of
    /// [`Actor::handle`](crate::Actor::handle), and a receive loop has no such
    /// point, so the body cannot call them. To end the actor, return from the
    /// body.
    ///
    /// Arty creates the body's future on the actor's worker, so the future need
    /// not be [`Send`]. It can keep an `Rc` or a `RefCell` across an `.await`.
    /// The body itself, with everything it captures, must be `Send`.
    ///
    /// When the body returns, the inbox drops its buffered messages, so their
    /// asks fail with [`AskError::is_canceled`](crate::AskError::is_canceled).
    /// The exit is [`ActorExit::Drained`] if a drain was in progress and the
    /// body read every buffered message. Otherwise the exit is
    /// [`ActorExit::Stopped`].
    pub fn spawn_with<M, F>(self, body: F) -> ActorWithPlacement<M>
    where
        M: Message,
        F: AsyncFnOnce(&mut Inbox<M>, &Context<M>) + Send + 'static,
    {
        ActorWithPlacement::new(self.runtime.dispatch_with(self.task_meta, self.capacity, body, None))
    }
}

impl<M: Message> ActorSpawner<'_, WithStream<M>> {
    /// Spawns the actor.
    pub fn spawn<A>(self, actor: A) -> ActorWithPlacement<M>
    where
        A: Actor<Message = M>,
    {
        ActorWithPlacement::new(self.runtime.dispatch(self.task_meta, self.capacity, actor, Some(self.stream.0)))
    }

    /// Spawns an actor written as a receive loop.
    ///
    /// [`Inbox::recv`] returns stream items like channel messages. The body
    /// runs as described in [`ActorSpawner::spawn_with`].
    pub fn spawn_with<F>(self, body: F) -> ActorWithPlacement<M>
    where
        F: AsyncFnOnce(&mut Inbox<M>, &Context<M>) + Send + 'static,
    {
        ActorWithPlacement::new(self.runtime.dispatch_with(self.task_meta, self.capacity, body, Some(self.stream.0)))
    }
}

impl<S> ActorSpawner<'_, S> {
    /// Sets the inbox capacity.
    #[must_use]
    pub const fn with_capacity(mut self, capacity: usize) -> Self {
        self.capacity = capacity;
        self
    }

    /// Attaches task metadata, such as a [`Placement`](crate::Placement), to the spawn.
    #[must_use]
    pub fn with_meta(mut self, meta: impl Into<TaskMeta>) -> Self {
        self.task_meta = Some(meta.into());
        self
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use arty::task::Context as TaskContext;

    use super::ActorRuntime;
    use crate::{Placement, PlacementToken};

    #[arty::test]
    async fn builder_uses_tuned_default_and_preserves_override(cx: TaskContext) {
        let runtime = ActorRuntime::from_builtins(&cx);

        assert_eq!(runtime.builder().capacity, 224);
        assert_eq!(runtime.builder().with_capacity(7).capacity, 7);
    }

    #[arty::test]
    async fn with_meta_accepts_placement(cx: TaskContext) {
        let runtime = ActorRuntime::from_builtins(&cx);
        assert!(runtime.builder().task_meta.is_none());

        let anywhere = runtime.builder().with_meta(Placement::Any);
        assert_eq!(anywhere.task_meta.unwrap().placement(), Placement::Any);

        let token = PlacementToken::from(cx.thread());
        let same_thread = runtime.builder().with_meta(Placement::SameThreadAs(token.clone()));
        assert_eq!(same_thread.task_meta.unwrap().placement(), Placement::SameThreadAs(token));
    }
}
