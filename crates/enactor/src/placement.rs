// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Worker placement for spawned actors and their tasks.

use arty::core::Thread;

/// Identifies the Arty worker thread that runs an actor.
///
/// Get a token from [`ActorWithPlacement::placement`](crate::ActorWithPlacement::placement),
/// or convert an Arty [`Thread`] with `From`. Pass it to
/// [`Placement::SameThreadAs`] to put related actors on the same worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacementToken(Thread);

impl PlacementToken {
    /// Returns the Arty worker thread of this token.
    #[must_use]
    pub const fn thread(&self) -> &Thread {
        &self.0
    }
}

impl From<Thread> for PlacementToken {
    fn from(thread: Thread) -> Self {
        Self(thread)
    }
}

impl From<&Thread> for PlacementToken {
    fn from(thread: &Thread) -> Self {
        Self(thread.clone())
    }
}

/// Selects the worker thread for a spawned actor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    /// A worker selected by the actor runtime. Successive spawns rotate through
    /// the workers of the Arty runtime.
    Any,
    /// The worker of the given token. The worker must belong to the same Arty
    /// runtime. Otherwise Arty rejects the task, and messages to the actor fail.
    SameThreadAs(PlacementToken),
}

/// Spawn options for an actor task.
///
/// Create it from a [`Placement`] with `From`, or pass a `Placement` directly
/// to [`ActorSpawner::with_meta`](crate::ActorSpawner::with_meta).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskMeta {
    placement: Placement,
}

impl TaskMeta {
    /// Returns the placement of the task.
    #[must_use]
    pub fn placement(&self) -> Placement {
        self.placement.clone()
    }
}

impl TaskMeta {
    pub(crate) fn into_placement(self) -> Placement {
        self.placement
    }
}

impl From<Placement> for TaskMeta {
    fn from(placement: Placement) -> Self {
        Self { placement }
    }
}
