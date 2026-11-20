// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware::ThreadAware;

use super::Hook;

/// A runtime hook callback and its relocatable data.
///
/// Arty clones the data for each worker and relocates each clone before the
/// callback runs there. Task-spawn hooks also keep one worker-neutral instance.
/// Neutral callbacks can run concurrently, so hook data must support shared
/// access. The callback is a function pointer, so it cannot capture data that
/// bypasses relocation.
#[derive(Debug)]
pub struct HookFn<H, D>
where
    H: Hook,
    D: ThreadAware + Clone + Sync + 'static,
{
    data: D,
    callback: fn(&mut H, &D),
}

impl<H, D> HookFn<H, D>
where
    H: Hook,
    D: ThreadAware + Clone + Sync + 'static,
{
    /// Creates a hook from relocatable data and a callback.
    pub const fn new(data: D, callback: fn(&mut H, &D)) -> Self {
        Self { data, callback }
    }
}

impl<H, D> Clone for HookFn<H, D>
where
    H: Hook,
    D: ThreadAware + Clone + Sync + 'static,
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
    D: ThreadAware + Clone + Sync + 'static,
{
    fn relocate(&mut self, source: Option<&thread_aware::Thread>, destination: &thread_aware::Thread) {
        self.data.relocate(source, destination);
    }
}

trait ErasedCallback<H: Hook>: Send + Sync + ThreadAware {
    fn clone_box(&self) -> Box<dyn ErasedCallback<H>>;
    fn execute(&self, event: &mut H);
}

impl<H, D> ErasedCallback<H> for HookFn<H, D>
where
    H: Hook,
    D: ThreadAware + Clone + Sync + 'static,
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

impl<H, D> From<HookFn<H, D>> for ErasedHookFn<H>
where
    H: Hook,
    D: ThreadAware + Clone + Sync + 'static,
{
    fn from(hook: HookFn<H, D>) -> Self {
        Self { callback: Box::new(hook) }
    }
}

impl<H: Hook> ErasedHookFn<H> {
    pub(crate) fn execute(&self, event: &mut H) {
        self.callback.execute(event);
    }
}

impl<H: Hook> Clone for ErasedHookFn<H> {
    fn clone(&self) -> Self {
        Self {
            callback: self.callback.clone_box(),
        }
    }
}
