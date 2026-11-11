// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware::ThreadAware;

use super::Hook;

pub(crate) type HookCallback<H, D> = fn(&mut H, &D);

trait Callback<H: Hook>: Send + ThreadAware {
    fn clone_box(&self) -> Box<dyn Callback<H>>;
    fn execute(&mut self, event: &mut H);
}

#[derive(ThreadAware)]
struct CallbackWithData<H: Hook, D>
where
    D: ThreadAware + Clone + 'static,
{
    data: D,
    #[thread_aware(skip)]
    callback: HookCallback<H, D>,
}

impl<H, D> Callback<H> for CallbackWithData<H, D>
where
    H: Hook,
    D: ThreadAware + Clone + 'static,
{
    fn clone_box(&self) -> Box<dyn Callback<H>> {
        Box::new(Self {
            data: self.data.clone(),
            callback: self.callback,
        })
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
    pub(crate) fn new<D>(data: D, callback: HookCallback<H, D>) -> Self
    where
        D: ThreadAware + Clone + 'static,
    {
        Self {
            callback: Box::new(CallbackWithData { data, callback }),
        }
    }

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
