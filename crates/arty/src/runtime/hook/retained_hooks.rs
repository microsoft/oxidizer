// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwap;

use super::{ErasedHookFn, HookRegistration, TaskObservers, TaskSpawn, WorkerSpawn};

pub(crate) struct ClearedHooks {
    _task_spawn: Arc<Vec<Arc<ErasedHookFn<TaskSpawn>>>>,
    _worker_spawn: Vec<ErasedHookFn<WorkerSpawn>>,
}

#[derive(derive_more::Debug)]
pub(crate) struct RetainedHooks {
    registration: Mutex<()>,
    closed: AtomicBool,
    has_task_spawn: AtomicBool,
    task_spawn: ArcSwap<Vec<Arc<ErasedHookFn<TaskSpawn>>>>,
    worker_spawn: Mutex<Vec<ErasedHookFn<WorkerSpawn>>>,
}

impl RetainedHooks {
    pub(crate) fn new(hooks: Vec<HookRegistration>) -> Self {
        let mut task_spawn = Vec::new();
        let mut worker_spawn = Vec::new();
        for hook in hooks {
            match hook {
                HookRegistration::TaskSpawn(hook) => task_spawn.push(Arc::new(hook)),
                HookRegistration::WorkerSpawn(hook) => worker_spawn.push(hook),
                HookRegistration::TaskPanic(_) => {}
            }
        }
        let has_task_spawn = !task_spawn.is_empty();
        Self {
            registration: Mutex::new(()),
            closed: AtomicBool::new(false),
            has_task_spawn: AtomicBool::new(has_task_spawn),
            task_spawn: ArcSwap::from_pointee(task_spawn),
            worker_spawn: Mutex::new(worker_spawn),
        }
    }

    pub(crate) fn push(&self, hook: HookRegistration) -> bool {
        let _registration = self.registration.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.closed.load(Ordering::Acquire) {
            return false;
        }
        match hook {
            HookRegistration::TaskSpawn(hook) => {
                Self::push_to(&self.task_spawn, hook);
                self.has_task_spawn.store(true, Ordering::Release);
            }
            HookRegistration::WorkerSpawn(hook) => self
                .worker_spawn
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(hook),
            HookRegistration::TaskPanic(_) => {}
        }
        true
    }

    fn push_to<T>(hooks: &ArcSwap<Vec<Arc<T>>>, hook: T) {
        let hook = Arc::new(hook);
        hooks.rcu(move |current| {
            let mut updated = current.as_ref().clone();
            updated.push(Arc::clone(&hook));
            Arc::new(updated)
        });
    }

    pub(crate) fn has_task_spawn(&self) -> bool {
        self.has_task_spawn.load(Ordering::Acquire)
    }

    pub(crate) fn execute_task_spawn(&self, create_event: impl FnOnce() -> TaskSpawn) -> Option<TaskObservers> {
        let hooks = self.task_spawn.load();
        if hooks.is_empty() {
            return None;
        }
        let mut event = create_event();
        for hook in hooks.iter() {
            hook.execute(&mut event);
        }
        event.into_observers()
    }

    pub(crate) fn clear(&self) -> ClearedHooks {
        let _registration = self.registration.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.closed.store(true, Ordering::Release);
        let cleared = ClearedHooks {
            _task_spawn: self.task_spawn.swap(Arc::new(Vec::new())),
            _worker_spawn: std::mem::take(&mut *self.worker_spawn.lock().unwrap_or_else(std::sync::PoisonError::into_inner)),
        };
        self.has_task_spawn.store(false, Ordering::Release);
        cleared
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.task_spawn
            .load()
            .len()
            .saturating_add(self.worker_spawn.lock().unwrap_or_else(std::sync::PoisonError::into_inner).len())
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::atomic::AtomicUsize;

    use thread_aware::{Thread, ThreadAware, ThreadBuilder};

    use super::*;
    use crate::runtime::hook::{HookFn, TaskPanic, registration};

    #[derive(Clone)]
    struct Calls(Arc<AtomicUsize>);

    impl ThreadAware for Calls {
        fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
    }

    #[test]
    fn initial_task_spawn_hooks_enable_the_global_check() {
        let task_spawn = registration(HookFn::<TaskSpawn, _>::new(
            Calls(Arc::new(AtomicUsize::new(0))),
            |_event, _calls| {},
        ));
        let worker_spawn = registration(HookFn::<WorkerSpawn, _>::new(
            Calls(Arc::new(AtomicUsize::new(0))),
            |_event, _calls| {},
        ));

        assert!(RetainedHooks::new(vec![task_spawn]).has_task_spawn());
        assert!(!RetainedHooks::new(vec![worker_spawn]).has_task_spawn());
    }

    #[test]
    fn publishes_executes_and_clears_snapshots() {
        let spawn_calls = Arc::new(AtomicUsize::new(0));
        let worker_spawn_calls = Arc::new(AtomicUsize::new(0));
        let hooks = RetainedHooks::new(Vec::new());
        let target = ThreadBuilder::default().build(std::thread::current().id());
        assert!(!hooks.has_task_spawn());
        let created = Cell::new(false);
        drop(hooks.execute_task_spawn(|| {
            created.set(true);
            TaskSpawn::new(None, target.clone(), std::panic::Location::caller())
        }));
        assert!(!created.get());

        hooks.has_task_spawn.store(true, Ordering::Release);
        assert!(
            hooks
                .execute_task_spawn(|| {
                    created.set(true);
                    TaskSpawn::new(None, target.clone(), std::panic::Location::caller())
                })
                .is_none()
        );
        assert!(!created.get());
        hooks.has_task_spawn.store(false, Ordering::Release);

        let panic_calls = Arc::new(AtomicUsize::new(0));
        let panic_hook = registration(HookFn::<TaskPanic, _>::new(Calls(Arc::clone(&panic_calls)), |_event, calls| {
            calls.0.fetch_add(1, Ordering::Relaxed);
        }));
        let panic_only = RetainedHooks::new(vec![panic_hook.clone()]);
        assert_eq!(panic_only.len(), 0);
        assert!(hooks.push(panic_hook));
        assert_eq!(hooks.len(), 0);

        assert!(hooks.push(registration(HookFn::<TaskSpawn, _>::new(
            Calls(Arc::clone(&spawn_calls)),
            |_event, calls| {
                calls.0.fetch_add(1, Ordering::Relaxed);
            },
        ))));
        assert!(hooks.has_task_spawn());
        assert!(hooks.push(registration(HookFn::<WorkerSpawn, _>::new(
            Calls(Arc::clone(&worker_spawn_calls)),
            |_event, calls| {
                calls.0.fetch_add(1, Ordering::Relaxed);
            },
        ))));
        drop(hooks.execute_task_spawn(|| TaskSpawn::new(None, target.clone(), std::panic::Location::caller())));
        drop(hooks.execute_task_spawn(|| TaskSpawn::new(None, target.clone(), std::panic::Location::caller())));
        assert_eq!(hooks.len(), 2);
        drop(hooks.clear());
        assert!(!hooks.has_task_spawn());
        assert!(!hooks.push(registration(HookFn::<WorkerSpawn, _>::new(
            Calls(Arc::clone(&worker_spawn_calls)),
            |_event, _calls| {},
        ))));
        assert_eq!(hooks.len(), 0);
        drop(hooks.execute_task_spawn(|| {
            created.set(true);
            TaskSpawn::new(None, target, std::panic::Location::caller())
        }));

        assert_eq!(spawn_calls.load(Ordering::Relaxed), 2);
        assert_eq!(worker_spawn_calls.load(Ordering::Relaxed), 0);
        assert_eq!(panic_calls.load(Ordering::Relaxed), 0);
        assert!(!created.get());
    }
}
