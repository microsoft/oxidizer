// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware::ThreadAware;

use super::{Hook, TaskPanic, TaskSpawn, WorkerSpawn};

/// A runtime hook callback and its relocatable data.
///
/// Arty clones the data for each worker and relocates each clone before the
/// callback runs there. Task-spawn hooks also keep one worker-neutral instance.
/// Registering a task-spawn hook therefore requires `D: Sync` because neutral
/// callbacks can run concurrently. Other hook kinds accept worker-local data
/// that is not `Sync`. The callback is a function pointer, so it cannot capture
/// data that bypasses relocation.
#[derive(Debug)]
pub struct HookFn<H, D>
where
    H: Hook,
    D: ThreadAware + Clone + 'static,
{
    data: D,
    callback: fn(&mut H, &D),
}

impl<H, D> HookFn<H, D>
where
    H: Hook,
    D: ThreadAware + Clone + 'static,
{
    /// Creates a hook from relocatable data and a callback.
    pub const fn new(data: D, callback: fn(&mut H, &D)) -> Self {
        Self { data, callback }
    }
}

impl<H, D> Clone for HookFn<H, D>
where
    H: Hook,
    D: ThreadAware + Clone + 'static,
{
    fn clone(&self) -> Self {
        Self {
            data: self.data.clone(),
            callback: self.callback,
        }
    }
}

impl<H, D> ThreadAware for HookFn<H, D>
where
    H: Hook,
    D: ThreadAware + Clone + 'static,
{
    fn relocate(&mut self, source: Option<&thread_aware::Thread>, destination: &thread_aware::Thread) {
        self.data.relocate(source, destination);
    }
}

trait ErasedCallback<H: Hook>: Send + ThreadAware {
    fn clone_box(&self) -> Box<dyn ErasedCallback<H>>;
    fn execute(&self, event: &mut H);
}

impl<H, D> ErasedCallback<H> for HookFn<H, D>
where
    H: Hook,
    D: ThreadAware + Clone + 'static,
{
    fn clone_box(&self) -> Box<dyn ErasedCallback<H>> {
        Box::new(self.clone())
    }

    fn execute(&self, event: &mut H) {
        (self.callback)(event, &self.data);
    }
}

#[derive(derive_more::Debug, ThreadAware)]
pub(crate) struct ErasedHookFn<H: Hook> {
    #[debug(ignore)]
    callback: Box<dyn ErasedCallback<H>>,
}

impl<H: Hook> ErasedHookFn<H> {
    fn new<D>(hook: HookFn<H, D>) -> Self
    where
        D: ThreadAware + Clone + 'static,
    {
        Self { callback: Box::new(hook) }
    }

    pub(crate) fn execute(&self, event: &mut H) {
        self.callback.execute(event);
    }
}

impl<D> From<HookFn<TaskPanic, D>> for ErasedHookFn<TaskPanic>
where
    D: ThreadAware + Clone + 'static,
{
    fn from(hook: HookFn<TaskPanic, D>) -> Self {
        Self::new(hook)
    }
}

impl<D> From<HookFn<WorkerSpawn, D>> for ErasedHookFn<WorkerSpawn>
where
    D: ThreadAware + Clone + 'static,
{
    fn from(hook: HookFn<WorkerSpawn, D>) -> Self {
        Self::new(hook)
    }
}

impl<D> From<HookFn<TaskSpawn, D>> for ErasedHookFn<TaskSpawn>
where
    D: ThreadAware + Clone + Sync + 'static,
{
    fn from(hook: HookFn<TaskSpawn, D>) -> Self {
        Self::new(hook)
    }
}

// SAFETY: TaskSpawn is the only hook retained as a worker-neutral shared
// instance. Its conversion requires D: Sync, and ErasedHookFn has no other
// constructors.
unsafe impl Sync for ErasedHookFn<TaskSpawn> {}

impl<H: Hook> Clone for ErasedHookFn<H> {
    fn clone(&self) -> Self {
        Self {
            callback: self.callback.clone_box(),
        }
    }
}
