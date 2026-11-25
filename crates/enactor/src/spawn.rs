// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Builder for actor spawn options.

use std::fmt;
use std::time::Duration;

use arty::task::JoinHandle;
use futures::future::{BoxFuture, Either, Shared, select};
use futures::{FutureExt, Stream};
use tick::Clock;

use crate::inbox::MessageStream;
use crate::runtime::{ActorRuntime, DEFAULT_CAPACITY};
use crate::{Actor, ActorRef, CompletionTimeoutError, Message, PlacementToken, TaskMeta};

/// Completion of an actor task, shared by every clone of its handle.
type Completion = Shared<BoxFuture<'static, ()>>;

/// Converts an Arty join handle into a completion that resumes an actor panic.
fn completion(task: JoinHandle<()>) -> Completion {
    async move {
        // A runtime that cancels or rejects the task still ends it. Only a panic
        // is reported to waiters.
        if let Err(error) = task.await
            && let Some(panic) = error.into_panic()
        {
            std::panic::resume_unwind(panic);
        }
    }
    .boxed()
    .shared()
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
    /// Reference to the spawned actor.
    #[must_use]
    pub const fn actor(&self) -> &ActorRef<M> {
        &self.actor
    }

    /// Worker-thread placement assigned to the actor.
    #[must_use]
    pub fn placement(&self) -> PlacementToken {
        self.placement.clone()
    }

    /// Waits up to `timeout` for actor task exit without requesting stop or drain.
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
    /// Resumes an actor task panic in the first waiter. Later waiters panic too.
    pub async fn wait_for_completion(&self, clock: &Clock, timeout: Duration) -> Result<(), CompletionTimeoutError> {
        let completion = self.completion.clone();
        let deadline = clock.delay(timeout);
        futures::pin_mut!(completion, deadline);
        match select(completion, deadline).await {
            Either::Left(_) => Ok(()),
            Either::Right(_) => Err(CompletionTimeoutError::new(timeout)),
        }
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
        let (actor_ref, placement, task) = self.runtime.dispatch(self.task_meta, self.capacity, actor, None);

        ActorWithPlacement {
            actor: actor_ref,
            placement,
            completion: completion(task),
        }
    }
}

impl<M: Message> ActorSpawner<'_, WithStream<M>> {
    /// Spawns the actor.
    pub fn spawn<A>(self, actor: A) -> ActorWithPlacement<M>
    where
        A: Actor<Message = M>,
    {
        let (actor_ref, placement, task) = self.runtime.dispatch(self.task_meta, self.capacity, actor, Some(self.stream.0));

        ActorWithPlacement {
            actor: actor_ref,
            placement,
            completion: completion(task),
        }
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
