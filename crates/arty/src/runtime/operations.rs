// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use performables::arc::Arc;
use performables::sync::once::OnceLock;
use thread_aware::Thread;

use crate::runtime::context::SharedState;
use crate::runtime::dispatch::DispatcherClient;
use crate::runtime::{Error, Runtime};
use crate::task::Builtins;

/// A handle for requesting shutdown and pinning external threads.
///
/// Create this handle with `RuntimeOperations::from(&runtime)` or
/// `RuntimeOperations::from(&builtins)`.
/// It can be cloned and used from any thread without keeping the runtime alive
/// or retaining an association with the task's worker.
///
/// [`request_stop`](Self::request_stop) initiates shutdown without waiting.
/// [`pin_current_thread_to`](Self::pin_current_thread_to) sets the calling thread's
/// processor affinity using an explicitly supplied worker coordinate.
#[derive(Debug, Clone)]
pub struct RuntimeOperations {
    dispatcher: DispatcherClient,
    shared_state: SharedState,
}

impl RuntimeOperations {
    /// Pins the calling thread to the supplied worker's processors.
    ///
    /// Use this when starting an external thread that should share a worker's
    /// processor locality. The worker coordinate must belong to this runtime.
    /// Pinning remains available after the runtime stops.
    ///
    /// Pinning does not make the thread an Arty worker or relocate its capabilities.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if `worker` belongs to another runtime, is not
    /// registered, or its processor services are unavailable.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::{Runtime, RuntimeOperations};
    ///
    /// let (operations, worker) = Runtime::new()?
    ///     .scheduler()
    ///     .block_on(async |cx| (RuntimeOperations::from(&cx), cx.thread().clone()))?;
    /// std::thread::scope(|scope| {
    ///     scope
    ///         .spawn(move || operations.pin_current_thread_to(&worker))
    ///         .join()
    ///         .expect("the callback only pins a registered worker")
    /// })?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[inline]
    pub fn pin_current_thread_to(&self, worker: &Thread) -> Result<(), Error> {
        if !self.dispatcher.owns(worker) {
            return Err(Error::new("the worker passed to pin_current_thread_to must belong to this runtime"));
        }
        let worker_index = self
            .dispatcher
            .worker_index(worker.id())
            .ok_or_else(|| Error::new("the worker passed to pin_current_thread_to must be a registered runtime worker"))?;
        let inner = self
            .shared_state
            .get(usize::from(worker_index))
            .and_then(OnceLock::get)
            .ok_or_else(|| Error::new("processor services for the worker passed to pin_current_thread_to are unavailable"))?;
        inner.processor_set.pin_current_thread_to();
        Ok(())
    }

    /// Requests shutdown without blocking the calling thread.
    ///
    /// May be called repeatedly from any thread. Pending tasks are cancelled,
    /// and new submissions receive [`JoinError`](crate::task::JoinError).
    /// Running blocking callbacks are allowed to finish. The owner remains
    /// responsible for waiting for shutdown when it is stopped or dropped.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::{Runtime, RuntimeOperations};
    ///
    /// let runtime = Runtime::new()?;
    /// let operations = RuntimeOperations::from(&runtime);
    /// operations.request_stop();
    /// runtime.stop()?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[cfg_attr(test, mutants::skip)] // It is impractical to test for "stuff not happening", so mutating this easily leads to timeouts.
    pub fn request_stop(&self) {
        self.dispatcher.stop();
    }
}

impl From<&Builtins> for RuntimeOperations {
    fn from(builtins: &Builtins) -> Self {
        Self {
            dispatcher: builtins.scheduler.dispatcher.clone(),
            shared_state: Arc::clone(&builtins.shared_state),
        }
    }
}

impl From<&Runtime> for RuntimeOperations {
    fn from(runtime: &Runtime) -> Self {
        Self {
            dispatcher: runtime.scheduler().dispatcher.clone(),
            shared_state: Arc::clone(&runtime.shared_state),
        }
    }
}

#[cfg(test)]
#[cfg(not(miri))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::thread;

    use many_cpus::SystemHardware;
    use observed::Sink;
    use thread_aware::{ThreadAware, ThreadBuilder, Unaware};
    use tick::runtime::InactiveClock;

    use super::*;
    use crate::runtime::bootstrap;
    use crate::runtime::config::{BlockingPoolPolicy, RuntimeConfig, WorkersPolicy};

    #[cfg_attr(test, mutants::skip)]
    fn runtime_with_coordinates(processors: usize) -> (Runtime, ThreadBuilder) {
        let coordinates = ThreadBuilder::default();
        let config = RuntimeConfig {
            workers_policy: WorkersPolicy::exactly(processors),
            blocking_pool_policy: BlockingPoolPolicy::shared(1),
            ..RuntimeConfig::default()
        };
        let runtime = bootstrap::build(config, &InactiveClock::default(), Sink::noop(), &coordinates).unwrap();
        (runtime, coordinates)
    }

    #[test]
    fn pinning_rejects_an_unregistered_thread() {
        let (runtime, coordinates) = runtime_with_coordinates(1);
        let operations = RuntimeOperations::from(&runtime);
        let unregistered = coordinates.build(thread::current().id());
        assert!(operations.pin_current_thread_to(&unregistered).is_err());
    }

    #[test]
    fn pinning_reports_unavailable_worker_services() {
        let (mut runtime, _) = runtime_with_coordinates(1);
        let worker = runtime
            .scheduler()
            .spawn_anywhere((), |cx, ()| async move { cx.thread().clone() })
            .join()
            .unwrap();
        runtime.shared_state = vec![OnceLock::new()].into();
        assert!(RuntimeOperations::from(&runtime).pin_current_thread_to(&worker).is_err());
    }

    #[test]
    fn repeated_and_unfamiliar_relocation_preserves_worker_services_and_explicit_pinning() {
        let (runtime, coordinates) = runtime_with_coordinates(1);
        let (source, mut scheduler, Unaware(operations), processor, mut builtins) = runtime
            .scheduler()
            .spawn_anywhere((), |cx, ()| async move {
                (
                    cx.thread().clone(),
                    cx.scheduler().clone(),
                    Unaware(RuntimeOperations::from(&cx)),
                    SystemHardware::current().current_processor_id(),
                    cx.clone(),
                )
            })
            .join()
            .unwrap();
        let unfamiliar = coordinates.build(thread::current().id());
        let foreign = ThreadBuilder::default().build(source.id());
        assert_eq!(source.owner(), unfamiliar.owner());
        assert_ne!(source.id(), unfamiliar.id());
        assert_ne!(source.owner(), foreign.owner());
        let blocking_thread = scheduler.spawn_blocking(|| thread::current().id()).join().unwrap();

        for destination in [&source, &unfamiliar, &foreign, &unfamiliar, &source] {
            scheduler.relocate(Some(destination), destination);
            builtins.relocate(Some(&source), destination);
            assert_eq!(builtins.thread(), &source);

            for scheduler in [&scheduler, builtins.scheduler()] {
                assert_eq!(scheduler.spawn(async |_| thread::current().id()).join().unwrap(), source.id());
                assert_eq!(scheduler.spawn_blocking(|| thread::current().id()).join().unwrap(), blocking_thread);
            }

            for operations in [operations.clone(), RuntimeOperations::from(&builtins)] {
                let source = source.clone();
                let observed = thread::spawn(move || {
                    operations.pin_current_thread_to(&source).unwrap();
                    let hardware = SystemHardware::current();
                    (hardware.is_thread_processor_pinned(), hardware.current_processor_id())
                })
                .join()
                .unwrap();
                assert_eq!(observed, (true, processor));
            }
        }
    }

    #[test]
    fn unpublished_worker_state_preserves_the_existing_binding() {
        if SystemHardware::current().processors().len() < 2 {
            eprintln!("requires two processors to exercise an unpublished destination worker");
            return;
        }
        let (runtime, _) = runtime_with_coordinates(2);
        let workers: Vec<_> = (0..2)
            .map(|_| {
                runtime
                    .scheduler()
                    .spawn_anywhere((), |cx, ()| async move { (cx.thread().clone(), cx) })
            })
            .map(|handle| handle.join().unwrap())
            .collect();
        let source = &workers[0].0;
        let destination = &workers[1].0;
        let mut builtins = workers[0].1.clone();

        let unpublished: SharedState = (0..workers.len()).map(|_| OnceLock::new()).collect();
        builtins.shared_state = unpublished;
        builtins.relocate(Some(source), destination);

        assert_eq!(builtins.thread(), source);
        assert_eq!(
            builtins.scheduler().spawn(async |_| thread::current().id()).join().unwrap(),
            source.id()
        );
    }
}
