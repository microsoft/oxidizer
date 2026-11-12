// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::RefCell;
use std::thread::LocalKey;

use thread_aware::{Thread, ThreadAware};

use super::{ErasedHookFn, Hook};

pub(crate) struct LocalHook<H: Hook> {
    pub(crate) hooks: &'static LocalKey<RefCell<Option<Vec<ErasedHookFn<H>>>>>,
}

impl<H: Hook> LocalHook<H> {
    pub(crate) fn initialize(&self) {
        self.hooks.with_borrow_mut(|hooks| {
            assert!(
                hooks.replace(Vec::new()).is_none(),
                "hooks are already installed on this task worker"
            );
        });
    }

    pub(crate) fn install(&self, mut hook: ErasedHookFn<H>, source: Option<&Thread>, destination: &Thread) {
        hook.relocate(source, destination);
        self.hooks.with_borrow_mut(|hooks| {
            hooks.as_mut().expect("a running worker retains its installed hooks").push(hook);
        });
    }

    pub(crate) fn execute(&self, event: &mut H) {
        self.hooks.with_borrow_mut(|hooks| {
            for hook in hooks.as_mut().expect("a running worker retains its installed hooks") {
                hook.execute(event);
            }
        });
    }

    pub(crate) fn clear(&self) {
        let hooks = self.hooks.with_borrow_mut(Option::take);
        drop(hooks);
    }
}
