// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware::ThreadAware;

use super::Hook;

type HookCallback<H, D> = fn(&mut H, &D);

/// A runtime hook callback and its worker-local data.
///
/// Arty clones the data for each worker and relocates it before the callback
/// runs. The callback is a function pointer, so it cannot capture data that
/// bypasses relocation.
pub struct HookFn<H, D>
where
    H: Hook,
    D: ThreadAware + Clone + 'static,
{
    data: D,
    callback: HookCallback<H, D>,
}

impl<H, D> HookFn<H, D>
where
    H: Hook,
    D: ThreadAware + Clone + 'static,
{
    /// Creates a hook from worker-local data and a callback.
    pub const fn new(data: D, callback: HookCallback<H, D>) -> Self {
        Self { data, callback }
    }

    pub(crate) fn erased(self) -> TypedHook<H> {
        TypedHook { callback: Box::new(self) }
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

trait Callback<H: Hook>: Send + ThreadAware {
    fn clone_box(&self) -> Box<dyn Callback<H>>;
    fn execute(&mut self, event: &mut H);
}

impl<H, D> Callback<H> for HookFn<H, D>
where
    H: Hook,
    D: ThreadAware + Clone + 'static,
{
    fn clone_box(&self) -> Box<dyn Callback<H>> {
        Box::new(self.clone())
    }

    fn execute(&mut self, event: &mut H) {
        (self.callback)(event, &self.data);
    }
}

#[derive(derive_more::Debug, ThreadAware)]
pub(crate) struct TypedHook<H: Hook> {
    #[debug(ignore)]
    callback: Box<dyn Callback<H>>,
}

impl<H: Hook> TypedHook<H> {
    pub(crate) fn execute(&mut self, event: &mut H) {
        self.callback.execute(event);
    }
}

impl<H: Hook> Clone for TypedHook<H> {
    fn clone(&self) -> Self {
        Self {
            callback: self.callback.clone_box(),
        }
    }
}
