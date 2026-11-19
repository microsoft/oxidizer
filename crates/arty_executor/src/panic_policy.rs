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
#[derive(Clone, derive_more::Debug)]
pub struct PanicPolicy(PanicPolicyKind);

#[derive(Clone, derive_more::Debug)]
enum PanicPolicyKind {
    Shutdown,
    Ignore,
    Dynamic(#[debug(ignore)] Rc<dyn Fn(TaskPanicPayload) -> (PanicAction, TaskPanicPayload)>),
}

impl PanicPolicy {
    /// Starts executor shutdown with a panic cause.
    pub const SHUTDOWN: Self = Self(PanicPolicyKind::Shutdown);

    /// Contains the failed task and continues running other tasks.
    pub const IGNORE: Self = Self(PanicPolicyKind::Ignore);

    /// Creates a policy that inspects each panic before selecting an action.
    ///
    /// The resolver runs synchronously on the executor thread. It must return
    /// the original panic payload and must not panic.
    pub fn dynamic(resolve: impl Fn(TaskPanicPayload) -> (PanicAction, TaskPanicPayload) + 'static) -> Self {
        Self(PanicPolicyKind::Dynamic(Rc::new(resolve)))
    }

    pub(crate) fn resolve(&self, panic: TaskPanicPayload) -> (PanicAction, TaskPanicPayload) {
        match &self.0 {
            PanicPolicyKind::Shutdown => (PanicAction::Shutdown, panic),
            PanicPolicyKind::Ignore => (PanicAction::Ignore, panic),
            PanicPolicyKind::Dynamic(resolve) => resolve(panic),
        }
    }
}

impl Default for PanicPolicy {
    fn default() -> Self {
        Self::SHUTDOWN
    }
}
