// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! How an actor ended.

use std::fmt;

/// How an actor ended.
///
/// Get it from [`ActorWithPlacement::exit`](crate::ActorWithPlacement::exit).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActorExit {
    /// A stop was requested with [`ActorRef::stop`](crate::ActorRef::stop) or
    /// [`Context::stop`](crate::Context::stop), or the actor loop ended on its own.
    /// A receive-loop body that returns while no drain is in progress also ends
    /// with this exit.
    Stopped,
    /// A drain finished. The actor handled every buffered message, then stopped.
    Drained,
    /// A message handler or a receive-loop body panicked.
    ///
    /// The Arty `TaskPanic` policy decides whether the runtime also shuts down.
    Panicked,
    /// The runtime canceled the actor task at shutdown, or rejected the task.
    ///
    /// For example, Arty rejects a task that is placed with a
    /// [`PlacementToken`](crate::PlacementToken) from another runtime.
    Aborted,
}

impl fmt::Display for ActorExit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Stopped => "actor stopped",
            Self::Drained => "actor drained its mailbox and stopped",
            Self::Panicked => "actor panicked",
            Self::Aborted => "runtime aborted the actor task",
        })
    }
}

/// How an actor loop returned.
///
/// The loop itself can only stop or drain. A panic or an abort happens outside
/// the loop, so this type cannot express one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoopExit {
    Stopped,
    Drained,
}

impl From<LoopExit> for ActorExit {
    fn from(exit: LoopExit) -> Self {
        match exit {
            LoopExit::Stopped => Self::Stopped,
            LoopExit::Drained => Self::Drained,
        }
    }
}
