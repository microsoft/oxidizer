// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::rc::Rc;

use crate::TaskPanicPayload;

/// The action selected after the executor contains a task panic.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PanicAction {
    /// Start executor shutdown with a panic cause.
    Shutdown,

    /// Contain the failed task and continue running other tasks.
    Ignore,
}

/// Selects how the executor responds to a contained task panic.
#[derive(Clone, derive_more::Debug, Default)]
pub enum PanicPolicy {
    /// Start executor shutdown with a panic cause.
    #[default]
    Shutdown,

    /// Contain the failed task and continue running other tasks.
    Ignore,

    /// Ask the caller how to respond to each contained task panic.
    ///
    /// The resolver runs synchronously on the executor thread. It receives and
    /// must return the original panic payload. It must not panic.
    Dynamic(#[debug(ignore)] Rc<dyn Fn(TaskPanicPayload) -> (PanicAction, TaskPanicPayload)>),
}

impl PanicPolicy {
    /// Creates a policy that inspects each panic before selecting an action.
    ///
    /// The resolver runs synchronously on the executor thread. It must return
    /// the original panic payload and must not panic.
    pub fn dynamic(resolve: impl Fn(TaskPanicPayload) -> (PanicAction, TaskPanicPayload) + 'static) -> Self {
        Self::Dynamic(Rc::new(resolve))
    }

    pub(crate) fn resolve(&self, panic: TaskPanicPayload) -> (PanicAction, TaskPanicPayload) {
        match self {
            Self::Shutdown => (PanicAction::Shutdown, panic),
            Self::Ignore => (PanicAction::Ignore, panic),
            Self::Dynamic(resolve) => resolve(panic),
        }
    }
}
