// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::RefCell;
use std::marker::PhantomData;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use arty_executor::TaskSet;
use observed::{Sink, emit};

use crate::runtime::telemetry::events::{PlacementLabel, TaskPanicked, TaskSpawned};
use crate::task::execution::prepare_local;
use crate::task::join::LocalJoinHandle;

thread_local! {
    // TaskSet contains a non-atomic weak reference and must also be dropped on its owner.
    static LOCAL_TASKS: RefCell<Option<RegisteredLocalTasks>> = const { RefCell::new(None) };
}

#[derive(Debug)]
struct RegisteredLocalTasks {
    identity: Arc<()>,
    tasks: Option<TaskSet>,
}

/// Keeps executor ownership local even when every `Builtins` handle leaves the worker.
#[derive(Debug)]
pub(crate) struct LocalTaskScope {
    _not_send: PhantomData<Rc<()>>,
}

impl LocalTaskScope {
    pub(crate) fn new() -> Self {
        LOCAL_TASKS.with_borrow_mut(|registered| {
            assert!(registered.is_none(), "a worker already owns this thread's local executor");
            *registered = Some(RegisteredLocalTasks {
                identity: Arc::new(()),
                tasks: None,
            });
        });
        Self { _not_send: PhantomData }
    }

    pub(crate) fn close() {
        let tasks =
            LOCAL_TASKS.with_borrow_mut(|registered| registered.as_mut().expect("local task scope is installed until drop").tasks.take());
        drop(tasks);
    }
}

impl Drop for LocalTaskScope {
    fn drop(&mut self) {
        drop(LOCAL_TASKS.with_borrow_mut(Option::take));
    }
}

/// Portable identity, with no thread-local executor state in its ownership graph.
#[derive(Debug, Clone)]
pub(crate) struct LocalTaskBinding {
    identity: Arc<()>,
    sink: Sink,
    shutdown: Arc<AtomicBool>,
}

impl LocalTaskBinding {
    pub(crate) fn new(tasks: TaskSet, sink: Sink, shutdown: Arc<AtomicBool>) -> Self {
        let identity = LOCAL_TASKS.with_borrow_mut(|registered| {
            let registered = registered.as_mut().expect("the worker owns a local executor scope");
            assert!(registered.tasks.is_none(), "local scheduler initialized more than once");
            registered.tasks = Some(tasks);
            Arc::clone(&registered.identity)
        });
        Self { identity, sink, shutdown }
    }

    pub(crate) fn local_scheduler(&self) -> Option<LocalTaskScheduler> {
        LOCAL_TASKS.with_borrow(|registered| {
            let registered = registered.as_ref()?;
            Arc::ptr_eq(&self.identity, &registered.identity).then(|| LocalTaskScheduler {
                binding: self.clone(),
                _not_send_sync: PhantomData,
            })
        })
    }
}

/// Scheduler for local, potentially non-`Send` tasks and results.
///
/// Obtain an owned scheduler through [`Builtins::local_scheduler`](crate::runtime::Builtins::local_scheduler)
/// on the associated worker. The scheduler is neither [`Send`] nor [`Sync`].
/// Cloning it does not keep the worker running.
///
/// A cancelled or rejected local task returns [`JoinError`](crate::task::JoinError)
/// with `is_shutdown() == true`.
/// See the [documentation guides](crate#documentation) for choosing
/// between local and cross-thread submission.
///
/// With the `macros` feature:
///
/// ```
/// use std::rc::Rc;
///
/// # #[cfg(feature = "macros")]
/// use arty::runtime::Builtins;
///
/// # #[cfg(feature = "macros")]
/// #[arty::main]
/// async fn main(cx: Builtins) {
///     let value = Rc::new(42);
///     let result = cx
///         .local_scheduler()
///         .expect("the task runs on its associated worker")
///         .spawn(async move || value)
///         .await
///         .expect("the local task completes before the entry point returns");
///     assert_eq!(*result, 42);
/// }
/// # #[cfg(not(feature = "macros"))] fn main() {}
/// ```
#[derive(Debug, Clone)]
pub struct LocalTaskScheduler {
    binding: LocalTaskBinding,
    _not_send_sync: PhantomData<Rc<()>>,
}

impl LocalTaskScheduler {
    fn tasks(&self) -> Option<TaskSet> {
        LOCAL_TASKS.with_borrow(|registered| {
            let registered = registered.as_ref().expect("local scheduler's worker is not running on this thread");
            assert!(
                Arc::ptr_eq(&self.binding.identity, &registered.identity),
                "local scheduler belongs to a different worker"
            );
            registered.tasks.clone()
        })
    }

    /// Starts a local task, creating its future immediately on the current worker.
    ///
    /// The factory takes no arguments. Captures and results may be non-`Send`,
    /// but still need to be `'static`; local spawning does not borrow the caller's stack.
    ///
    /// After shutdown starts, returns a join that is immediately ready with a
    /// shutdown error, without invoking the factory.
    ///
    /// # Panics
    ///
    /// Panics outside the token's own worker-local context while the runtime is
    /// running. Factory and future panics are returned through the join.
    pub fn spawn<FF, F, R>(&self, future_factory: FF) -> LocalJoinHandle<R>
    where
        FF: FnOnce() -> F + 'static,
        F: Future<Output = R> + 'static,
        R: 'static,
    {
        if self.binding.shutdown.load(Ordering::Acquire) {
            return LocalJoinHandle::shutdown();
        }
        // Release the TLS borrow before invoking user code, which may spawn recursively.
        let Some(tasks) = self.tasks() else {
            return LocalJoinHandle::shutdown();
        };
        let future = match catch_unwind(AssertUnwindSafe(future_factory)) {
            Ok(future) => future,
            Err(payload) => {
                emit!(&self.binding.sink, TaskPanicked);
                return LocalJoinHandle::panicked(payload);
            }
        };
        let parent = self.binding.sink.transfer_context();
        let (future, handle) = prepare_local(future, parent, self.binding.sink.clone());
        emit!(
            &self.binding.sink,
            TaskSpawned {
                placement: PlacementLabel("local")
            }
        );
        drop(tasks.add(future));
        handle
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use std::cell::Cell;
    use std::thread;

    use super::*;

    #[test]
    fn local_scheduler_tokens_are_thread_confined_but_bindings_are_portable() {
        static_assertions::assert_not_impl_any!(LocalTaskScheduler: Send, Sync);
        static_assertions::assert_impl_all!(LocalTaskScheduler: Clone);
        static_assertions::assert_not_impl_any!(LocalTaskScope: Send, Sync);
        static_assertions::assert_impl_all!(LocalTaskBinding: Send, Sync, Clone);
    }

    fn scope_binding() -> LocalTaskBinding {
        LOCAL_TASKS.with_borrow(|registered| LocalTaskBinding {
            identity: Arc::clone(&registered.as_ref().unwrap().identity),
            sink: Sink::noop(),
            shutdown: Arc::new(AtomicBool::new(false)),
        })
    }

    #[test]
    fn local_scope_can_be_recreated_on_the_same_thread() {
        let scope = LocalTaskScope::new();
        drop(scope);
        let _replacement = LocalTaskScope::new();
    }

    #[test]
    fn portable_binding_can_leave_and_reenter_its_scope() {
        let _scope = LocalTaskScope::new();
        let binding = scope_binding();
        let binding = thread::spawn(move || {
            assert!(binding.local_scheduler().is_none());
            drop(binding.clone());
            binding
        })
        .join()
        .unwrap();
        assert!(binding.local_scheduler().is_some());
    }

    #[test]
    fn portable_binding_cannot_acquire_a_token_from_another_scope() {
        let _scope = LocalTaskScope::new();
        let binding = scope_binding();
        thread::spawn(move || {
            let _other_scope = LocalTaskScope::new();
            assert!(binding.local_scheduler().is_none());
        })
        .join()
        .unwrap();
    }

    #[test]
    fn portable_bindings_can_be_dropped_during_and_after_scope_shutdown() {
        let scope = LocalTaskScope::new();
        let binding = scope_binding();
        let retained = binding.clone();
        thread::scope(|threads| {
            threads.spawn(move || drop(binding));
            drop(scope);
        });
        thread::spawn(move || assert!(retained.local_scheduler().is_none())).join().unwrap();
    }

    fn assert_rejected_before_factory(scheduler: &LocalTaskScheduler) {
        let invoked = Rc::new(Cell::new(false));
        let captured = Rc::clone(&invoked);
        let result = catch_unwind(AssertUnwindSafe(|| {
            scheduler.spawn(move || {
                captured.set(true);
                async {}
            })
        }));
        result.unwrap_err();
        assert!(!invoked.get());
    }

    #[test]
    fn retained_local_tokens_do_not_keep_the_scope_alive() {
        let scope = LocalTaskScope::new();
        let binding = scope_binding();
        let scheduler = binding.local_scheduler().unwrap();
        drop(scope);
        assert!(binding.local_scheduler().is_none());
        assert_rejected_before_factory(&scheduler);
    }

    #[test]
    fn retained_local_tokens_cannot_submit_to_a_replacement_scope() {
        let scope = LocalTaskScope::new();
        let binding = scope_binding();
        let scheduler = binding.local_scheduler().unwrap();
        drop(scope);
        let _replacement = LocalTaskScope::new();
        assert!(binding.local_scheduler().is_none());
        assert!(scope_binding().local_scheduler().is_some());
        assert_rejected_before_factory(&scheduler);
    }
}
