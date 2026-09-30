// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use many_cpus::ProcessorSet;
use thread_aware::{Thread, ThreadAware};

use crate::runtime::context::init::SharedState;
use crate::runtime::dispatch::DispatcherClient;

/// A handle for requesting shutdown and pinning external threads.
///
/// Obtain a handle through
/// [`Builtins::runtime_operations`](crate::runtime::Builtins::runtime_operations).
/// It can be cloned and used from any thread without keeping the runtime alive.
///
/// [`stop`](Self::stop) requests shutdown.
/// [`pin_current_thread`](Self::pin_current_thread) sets the calling thread's
/// processor affinity using the associated worker's processor set. Cloning
/// preserves that set; relocation to an initialized worker of the same runtime
/// updates it. Foreign or unregistered destinations leave it unchanged.
#[derive(Debug, Clone)]
pub struct RuntimeOperations {
    dispatcher: DispatcherClient,
    thread: Thread,
    processors: ProcessorSet,
    shared_state: SharedState,
}

impl RuntimeOperations {
    pub(in crate::runtime) fn new(
        dispatcher: DispatcherClient,
        thread: Thread,
        processors: ProcessorSet,
        shared_state: SharedState,
    ) -> Self {
        Self {
            dispatcher,
            thread,
            processors,
            shared_state,
        }
    }

    /// Pins the calling thread to the associated worker's processors.
    ///
    /// Use this when starting an external thread that should share a worker's
    /// processor locality. The handle retains a processor snapshot, so pinning
    /// remains available after the runtime stops. Clone it before moving it into
    /// a thread-start callback to preserve that callback's processor selection.
    ///
    /// Pinning does not make the thread an Arty worker or relocate its capabilities.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::Runtime;
    ///
    /// let operations = Runtime::new()?.run(async |cx| cx.runtime_operations().clone())?;
    /// std::thread::scope(|scope| {
    ///     scope.spawn(move || operations.pin_current_thread());
    /// });
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[inline]
    pub fn pin_current_thread(&self) {
        self.processors.pin_current_thread_to();
    }

    /// Requests shutdown without blocking the calling thread.
    ///
    /// May be called repeatedly from any thread. Pending tasks are cancelled,
    /// and new submissions receive [`JoinError`](crate::task::JoinError).
    /// Running blocking callbacks are allowed to finish. See
    /// [`Runtime::stop`](crate::runtime::Runtime::stop) for the full shutdown contract.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::Runtime;
    ///
    /// let runtime = Runtime::new()?;
    /// let operations = runtime
    ///     .task_scheduler()
    ///     .spawn(async |cx| cx.runtime_operations().clone())
    ///     .wait()?;
    /// operations.stop();
    /// runtime.wait();
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[cfg_attr(test, mutants::skip)] // It is impractical to test for "stuff not happening", so mutating this easily leads to timeouts.
    pub fn stop(&self) {
        self.dispatcher.stop();
    }

    /// Rebinds to a worker whose processor state has already been resolved.
    pub(crate) fn relocate_to_worker(&mut self, destination: &Thread, processors: ProcessorSet) {
        self.processors = processors;
        self.thread = destination.clone();
    }
}

impl ThreadAware for RuntimeOperations {
    fn relocate(&mut self, _source: Option<&Thread>, destination: &Thread) {
        if self.thread == *destination || self.thread.owner() != destination.owner() {
            return;
        }
        let Some(worker_index) = self.dispatcher.worker_index(destination.id()) else {
            return;
        };
        let Some(inner) = self
            .shared_state
            .get(usize::from(worker_index))
            .expect("registered runtime worker must have a shared-state slot")
            .get()
        else {
            return;
        };
        let processors = inner.processor_set.clone();
        self.relocate_to_worker(destination, processors);
    }
}

#[cfg(test)]
#[cfg(not(miri))]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use std::sync::{Arc, OnceLock};
    use std::thread;

    use many_cpus::SystemHardware;
    use observed::Sink;
    use thread_aware::ThreadBuilder;
    use tick::runtime::InactiveClock;

    use super::*;
    use crate::runtime::bootstrap;
    use crate::runtime::config::{BlockingPoolPolicy, ProcessorCount, RuntimeConfig};
    use crate::runtime::handle::Runtime;

    #[cfg_attr(test, mutants::skip)]
    fn runtime_with_coordinates(processors: usize) -> (Runtime, ThreadBuilder) {
        let coordinates = ThreadBuilder::default();
        let config = RuntimeConfig {
            num_processors: ProcessorCount::exactly(processors),
            blocking_pool_policy: BlockingPoolPolicy::shared(1),
            ..RuntimeConfig::default()
        };
        let runtime = bootstrap::build(config, &InactiveClock::default(), Sink::noop(), &coordinates).unwrap();
        (runtime, coordinates)
    }

    #[test]
    fn repeated_and_unfamiliar_relocation_preserves_all_handle_bindings() {
        let (runtime, coordinates) = runtime_with_coordinates(1);
        let (source, mut scheduler, mut operations, processor, mut builtins) = runtime
            .task_scheduler()
            .spawn(async |cx| {
                (
                    cx.thread().clone(),
                    cx.scheduler().clone(),
                    cx.runtime_operations().clone(),
                    SystemHardware::current().current_processor_id(),
                    cx.clone(),
                )
            })
            .wait()
            .unwrap();
        let unfamiliar = coordinates.build(thread::current().id());
        let foreign = ThreadBuilder::default().build(source.id());
        assert_eq!(source.owner(), unfamiliar.owner());
        assert_ne!(source.id(), unfamiliar.id());
        assert_ne!(source.owner(), foreign.owner());
        let blocking_thread = scheduler.spawn_blocking(|| thread::current().id()).wait().unwrap();

        for destination in [&source, &unfamiliar, &foreign, &unfamiliar, &source] {
            operations.relocate(None, destination);
            scheduler.relocate(Some(destination), destination);
            builtins.relocate(Some(&source), destination);

            assert_eq!(operations.thread, source);
            assert_eq!(builtins.runtime_operations().thread, source);
            assert_eq!(builtins.thread(), &source);

            for scheduler in [&scheduler, builtins.scheduler()] {
                let task = scheduler.spawn(async |_| thread::current().id());
                assert_eq!(task.wait().unwrap(), source.id());
                assert_eq!(scheduler.spawn_blocking(|| thread::current().id()).wait().unwrap(), blocking_thread);
            }

            for operations in [operations.clone(), builtins.runtime_operations().clone()] {
                let observed = thread::spawn(move || {
                    operations.pin_current_thread();
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
            .map(|_| runtime.task_scheduler().spawn(async |cx| (cx.thread().clone(), cx)))
            .map(|handle| handle.wait().unwrap())
            .collect();
        let source = &workers[0].0;
        let destination = &workers[1].0;
        let mut builtins = workers[0].1.clone();
        let mut operations = builtins.runtime_operations().clone();

        // Model the interval before the destination has published its worker-local services.
        let unpublished: SharedState = (0..workers.len()).map(|_| OnceLock::new()).collect();
        builtins.shared_state = Arc::clone(&unpublished);
        operations.shared_state = unpublished;

        operations.relocate(Some(source), destination);
        builtins.relocate(Some(source), destination);

        assert_eq!(&operations.thread, source);
        assert_eq!(&builtins.runtime_operations().thread, source);
        assert_eq!(builtins.thread(), source);
        let task = builtins.scheduler().spawn(async |_| thread::current().id());
        assert_eq!(task.wait().unwrap(), source.id());
    }
}
