// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Actor spawning and execution on Arty.
//!
//! [`ActorRuntime`] owns the default spawn path, placement selection, and actor
//! loop used by Enactor.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use arty::core::{Thread, ThreadAware};
use arty::task::{Context as TaskContext, JoinHandle};
use thread_aware::{Unaware, unaware};
use tick::{Clock, Timer};

use crate::exit::LoopExit;
use crate::inbox::{Channel, MessageStream};
use crate::spawn::{ActorSpawner, ActorWithPlacement};
use crate::{Actor, ActorExit, ActorId, ActorRef, Context, Inbox, Placement, PlacementToken, TaskMeta};

/// Values moved into the task that runs one actor.
pub(crate) type TaskData<A> = Unaware<(Inbox<<A as Actor>::Message>, A, ActorRef<<A as Actor>::Message>)>;

/// Default capacity for actor inboxes.
pub(crate) const DEFAULT_CAPACITY: usize = 224;

/// Runtime used to spawn actors onto the Arty scheduler.
///
/// Clones share the same Arty runtime and the same worker rotation for
/// [`Placement::Any`].
#[derive(Debug, Clone)]
pub struct ActorRuntime {
    cx: TaskContext,
    next_worker: Arc<AtomicUsize>,
}

impl ActorRuntime {
    /// Builds an [`ActorRuntime`] from the context of the current Arty task.
    ///
    /// Actors spawned without a placement run on the worker of `cx`.
    #[must_use]
    pub fn from_builtins(cx: &TaskContext) -> Self {
        Self {
            cx: cx.clone(),
            next_worker: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Spawns an actor on the current core.
    pub fn spawn<A>(&self, actor: A) -> ActorRef<A::Message>
    where
        A: Actor,
    {
        self.dispatch(None, DEFAULT_CAPACITY, actor, None).0
    }

    /// Spawns an actor on a runtime-selected core and returns its placement token.
    ///
    /// Use the token to co-locate related actors or Arty tasks on the same worker thread.
    pub fn spawn_anywhere<A>(&self, actor: A) -> ActorWithPlacement<A::Message>
    where
        A: Actor,
    {
        self.builder().with_meta(Placement::Any).spawn(actor)
    }

    /// Starts an [`ActorSpawner`] for non-default placement or inbox capacity.
    #[must_use]
    pub const fn builder(&self) -> ActorSpawner<'_> {
        ActorSpawner::new(self)
    }

    /// Spawns an actor with the selected placement, inbox capacity, and an
    /// optional stream merged into its inbox.
    pub(crate) fn dispatch<A>(
        &self,
        task_meta: Option<TaskMeta>,
        capacity: usize,
        actor: A,
        stream: Option<MessageStream<A::Message>>,
    ) -> (ActorRef<A::Message>, PlacementToken, JoinHandle<Unaware<LoopExit>>)
    where
        A: Actor,
    {
        let (message_tx, message_rx) = Channel::bounded(capacity);
        let (control_tx, control_rx) = Channel::unbounded();
        let actor_ref: ActorRef<<A as Actor>::Message> = ActorRef::new(ActorId::next(), message_tx, control_tx);
        let inbox = Inbox::new(message_rx, control_rx, stream);
        let data: TaskData<A> = unaware((inbox, actor, actor_ref.clone()));

        let (placement, completion) = self.spawn_internal_task(task_meta, data, Self::run::<A>);

        (actor_ref, placement, completion)
    }

    /// Sends a message to an actor after every interval.
    ///
    /// The timer runs on the actor's worker thread. Each tick waits for mailbox
    /// capacity before the timer starts its next interval.
    pub fn send_interval<M, F>(&self, actor: &ActorWithPlacement<M>, clock: &Clock, interval: Duration, make_message: F)
    where
        M: crate::Message,
        F: Fn() -> M + Send + 'static,
    {
        let data = (clock.clone(), unaware((interval, make_message, actor.actor().clone())));
        // Dropping the join handle does not cancel the timer task.
        let _ = self.spawn_internal_task(
            Some(Placement::SameThreadAs(actor.placement()).into()),
            data,
            Self::run_interval::<M, F>,
        );
    }

    /// Tells `supervisor` how `actor` ended, once it ends.
    ///
    /// `notify` turns the [`ActorExit`] into a supervisor message. The notice
    /// waits for mailbox capacity, so a full supervisor mailbox delays it but
    /// does not drop it. If the supervisor mailbox is closed, the notice is
    /// dropped, because no actor is left to receive it.
    ///
    /// Each call sends exactly one notice, even when `actor` has already ended.
    /// Two calls on the same actor send two notices. The monitor is a task on
    /// the worker of this runtime, and it ends after it sends its notice.
    pub fn monitor<M, S, F>(&self, actor: &ActorWithPlacement<M>, supervisor: &ActorRef<S>, notify: F)
    where
        M: crate::Message,
        S: crate::Message,
        F: FnOnce(ActorExit) -> S + Send + 'static,
    {
        let data = unaware((actor.exit(), supervisor.clone(), notify));
        // Dropping the join handle does not cancel the monitor task.
        let _ = self.spawn_internal_task(None, data, Self::run_monitor);
    }

    /// Spawns an internal runtime task with the same placement options as actors.
    pub(crate) fn spawn_internal_task<D, F, R>(
        &self,
        task_meta: Option<TaskMeta>,
        data: D,
        run: fn(D) -> F,
    ) -> (PlacementToken, JoinHandle<R>)
    where
        D: ThreadAware + Send + 'static,
        F: Future<Output = R> + 'static,
        R: ThreadAware + Send + 'static,
    {
        let thread = self.select_worker(task_meta);
        let scheduler = self.cx.scheduler();
        let completion = if thread == *self.cx.thread() {
            scheduler.spawn(move |_| run(data))
        } else {
            scheduler.spawn_at(&thread, data, run)
        };

        (PlacementToken::from(thread), completion)
    }

    fn select_worker(&self, task_meta: Option<TaskMeta>) -> Thread {
        match task_meta.map(TaskMeta::into_placement) {
            None => self.cx.thread().clone(),
            Some(Placement::SameThreadAs(token)) => token.thread().clone(),
            Some(Placement::Any) => {
                let mut workers = self.cx.control().workers();
                let index = self.next_worker.fetch_add(1, Ordering::Relaxed) % workers.len();
                workers
                    .nth(index)
                    .expect("the index is reduced modulo the snapshot length, and a snapshot is never empty")
            }
        }
    }

    async fn run_interval<M, F>(data: (Clock, Unaware<(Duration, F, ActorRef<M>)>))
    where
        M: crate::Message,
        F: Fn() -> M + Send + 'static,
    {
        let (clock, timer_data) = data;
        let (interval, make_message, actor) = timer_data.into_inner();
        let mut timer = Timer::new(&clock, interval);

        loop {
            let _tick = timer.next().await;
            if actor.tell(make_message()).await.is_err() {
                break;
            }
        }
    }

    async fn run_monitor<E, S, F>(data: Unaware<(E, ActorRef<S>, F)>)
    where
        E: Future<Output = ActorExit>,
        S: crate::Message,
        F: FnOnce(ActorExit) -> S,
    {
        let (exit, supervisor, notify) = data.into_inner();
        let notice = notify(exit.await);

        if let Err(closed) = supervisor.tell(notice).await {
            // The supervisor mailbox is closed, so no actor is left to read the notice.
            drop(closed);
        }
    }

    /// Runs the actor loop until the inbox closes or the actor requests stop.
    pub(crate) async fn run<A>(data: TaskData<A>) -> Unaware<LoopExit>
    where
        A: Actor,
    {
        let (mut inbox, mut actor, actor_ref) = data.into_inner();
        let mut ctx = Context::new(actor_ref);

        while let Ok(message) = inbox.recv().await {
            actor.handle(message, &mut ctx).await;

            if ctx.apply(&inbox) {
                break;
            }
        }

        unaware(inbox.exit())
    }
}

impl ThreadAware for ActorRuntime {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        self.cx.relocate(source, destination);
    }
}
