// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::RefCell;
use std::thread::LocalKey;

use thread_aware::{Thread, ThreadAware};

use super::{ErasedHookFn, Hook};

#[derive(Debug)]
pub(crate) struct LocalHook<H: Hook> {
    pub(crate) hooks: &'static LocalKey<RefCell<Option<Vec<ErasedHookFn<H>>>>>,
}

pub(crate) type LocalHookGuard<H> = scopeguard::ScopeGuard<&'static LocalHook<H>, fn(&'static LocalHook<H>)>;

impl<H: Hook> LocalHook<H> {
    pub(crate) fn initialize(&'static self) -> LocalHookGuard<H> {
        self.hooks.with_borrow_mut(|hooks| {
            assert!(
                hooks.replace(Vec::new()).is_none(),
                "hooks are already installed on this task worker"
            );
        });
        let clear: fn(&'static Self) = |hook| {
            _ = hook.hooks.try_with(|hooks| {
                let hooks = hooks.borrow_mut().take();
                drop(hooks);
            });
        };
        scopeguard::guard(self, clear)
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::hook::Panic;

    thread_local! {
        static HOOKS: RefCell<Option<Vec<ErasedHookFn<Panic>>>> = const { RefCell::new(None) };
    }

    static LOCAL_HOOK: LocalHook<Panic> = LocalHook { hooks: &HOOKS };

    #[test]
    fn guard_initializes_and_clears_worker_local_storage() {
        HOOKS.with_borrow_mut(|hooks| *hooks = None);

        let guard = LOCAL_HOOK.initialize();
        assert!(HOOKS.with_borrow(Option::is_some));

        drop(guard);
        assert!(HOOKS.with_borrow(Option::is_none));
    }
}
