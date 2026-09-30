// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use many_cpus::ProcessorSet;
use thread_aware::{Thread, ThreadAware};

use crate::runtime::context::init::SharedState;
use crate::runtime::dispatch::DispatcherClient;

/// Thread-aware runtime operations and worker processor snapshots.
///
/// This handle can be used from any thread. Relocation to an initialized worker of its
/// owning runtime changes the associated worker. An unfamiliar destination retains its binding.
/// Cloning or retaining it does not prevent the runtime owner from shutting down.
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

    /// Pins the current thread to the processors assigned to this handle's worker.
    ///
    /// This uses a captured processor snapshot and remains usable after the runtime stops.
    /// Clone the handle before capturing it in a thread-start callback to keep that
    /// callback's processor selection independent of subsequent relocation.
    ///
    /// ```
    /// use std::num::NonZeroUsize;
    ///
    /// use arty::runtime::{ProcessorCount, Runtime};
    ///
    /// let operations = Runtime::builder()
    ///     .processor_count(ProcessorCount::at_most(NonZeroUsize::MIN))
    ///     .build()
    ///     .unwrap()
    ///     .run(async |cx| cx.runtime_operations().clone());
    /// std::thread::spawn(move || operations.pin_current_thread())
    ///     .join()
    ///     .unwrap();
    /// ```
    #[inline]
    pub fn pin_current_thread(&self) {
        self.processors.pin_current_thread_to();
    }

    #[doc = include_str!("../../../docs/snippets/fn_runtime_stop.md")]
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
    use std::num::NonZeroUsize;
    use std::sync::{Arc, OnceLock};
    use std::thread;

    use many_cpus::SystemHardware;
    use observed::Sink;
    use thread_aware::ThreadBuilder;
    use tick::runtime::InactiveClock;

    use super::*;
    use crate::runtime::bootstrap;
    use crate::runtime::config::{ProcessorCount, RuntimeConfig, WorkerPoolPolicy};
    use crate::runtime::handle::Runtime;
    use crate::task::join::JoinHandle;

    #[cfg_attr(test, mutants::skip)]
    fn runtime_with_coordinates(processors: NonZeroUsize) -> (Runtime, ThreadBuilder) {
        let coordinates = ThreadBuilder::default();
        let config = RuntimeConfig {
            num_processors: ProcessorCount::exactly(processors),
            worker_pool_policy: WorkerPoolPolicy::shared(1),
            ..RuntimeConfig::default()
        };
        let runtime = bootstrap::build(config, &InactiveClock::default(), Sink::noop(), &coordinates).unwrap();
        (runtime, coordinates)
    }

    #[test]
    fn repeated_and_unfamiliar_relocation_preserves_all_handle_bindings() {
        let (runtime, coordinates) = runtime_with_coordinates(NonZeroUsize::MIN);
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
            .wait();
        let unfamiliar = coordinates.build(thread::current().id());
        let foreign = ThreadBuilder::default().build(source.id());
        assert_eq!(source.owner(), unfamiliar.owner());
        assert_ne!(source.id(), unfamiliar.id());
        assert_ne!(source.owner(), foreign.owner());
        let system_thread = scheduler.spawn_system(|| thread::current().id()).wait();

        for destination in [&source, &unfamiliar, &foreign, &unfamiliar, &source] {
            operations.relocate(None, destination);
            scheduler.relocate(Some(destination), destination);
            builtins.relocate(Some(&source), destination);

            assert_eq!(operations.thread, source);
            assert_eq!(builtins.runtime_operations().thread, source);
            assert_eq!(builtins.thread(), &source);

            for scheduler in [&scheduler, builtins.scheduler()] {
                let task = scheduler.spawn(async |_| thread::current().id());
                assert_eq!(task.wait(), source.id());
                assert_eq!(scheduler.spawn_system(|| thread::current().id()).wait(), system_thread);
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
        let (runtime, _) = runtime_with_coordinates(NonZeroUsize::new(2).unwrap());
        let workers: Vec<_> = (0..2)
            .map(|_| runtime.task_scheduler().spawn(async |cx| (cx.thread().clone(), cx)))
            .map(JoinHandle::wait)
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
        assert_eq!(task.wait(), source.id());
    }
}
