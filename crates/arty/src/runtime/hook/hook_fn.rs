// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware::ThreadAware;

use super::Hook;

/// A runtime hook callback and its worker-local data.
///
/// Arty clones the data for each worker and relocates it before the callback
/// runs. The callback is a function pointer, so it cannot capture data that
/// bypasses relocation.
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
    /// Creates a hook from worker-local data and a callback.
    pub const fn new(data: D, callback: fn(&mut H, &D)) -> Self {
        Self { data, callback }
    }

    pub(crate) fn erased(self) -> ErasedHookFn<H> {
        ErasedHookFn { callback: Box::new(self) }
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

impl<H> From<fn(&mut H, &())> for HookFn<H, ()>
where
    H: Hook,
{
    fn from(callback: fn(&mut H, &())) -> Self {
        Self::new((), callback)
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
    fn execute(&mut self, event: &mut H);
}

impl<H, D> ErasedCallback<H> for HookFn<H, D>
where
    H: Hook,
    D: ThreadAware + Clone + 'static,
{
    fn clone_box(&self) -> Box<dyn ErasedCallback<H>> {
        Box::new(self.clone())
    }

    fn execute(&mut self, event: &mut H) {
        (self.callback)(event, &self.data);
    }
}

#[derive(derive_more::Debug, ThreadAware)]
pub struct ErasedHookFn<H: Hook> {
    #[debug(ignore)]
    callback: Box<dyn ErasedCallback<H>>,
}

impl<H: Hook> ErasedHookFn<H> {
    pub(crate) fn execute(&mut self, event: &mut H) {
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
