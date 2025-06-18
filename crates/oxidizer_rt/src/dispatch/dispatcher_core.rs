// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;

use isolated_domains::Domain;
use nonempty::NonEmpty;

use crate::workers::AsyncWorkerCommand;
use crate::{
    Instantiation, Placement, PlacementToken, RemoteJoinHandle, RuntimeThreadState, SpawnInstance,
    WaitForShutdown, WakerFacade, once_event,
};

/// Directs commands to specific workers.
///
/// The dispatcher is aware of all workers that exist, their association with different hardware
/// resources, and what sort of commands are appropriate for which workers. It sends commands to
/// the relevant workers when commanded to by the caller.
///
/// This is used by:
/// 1. `Runtime` to send commands to workers from arbitrary code.
/// 2. Task-specific context objects (e.g. `TaskContext`) for the same purpose.
///
/// Commands are received as function calls to the dispatcher and delivered via message channels.
#[derive(Debug)]
pub struct DispatcherCore<WFS, TS> {
    wait_for_shutdown: WFS,

    /// We record whether shutdown has started, both to avoid double-shutdown and to execute
    /// special-case logic in some situations that need special handling during shutdown.
    shutdown_started: AtomicBool,

    async_command_txs: NonEmpty<(mpsc::Sender<AsyncWorkerCommand<TS>>, WakerFacade, Domain)>,

    // Minimal effort round-robin scheduling of async tasks.
    next_async_worker_index: AtomicUsize,
}

impl<WFS, TS> DispatcherCore<WFS, TS>
where
    TS: RuntimeThreadState,
{
    pub const fn new(
        wait_for_shutdown: WFS,
        async_command_txs: NonEmpty<(mpsc::Sender<AsyncWorkerCommand<TS>>, WakerFacade, Domain)>,
    ) -> Self {
        Self {
            wait_for_shutdown,
            shutdown_started: AtomicBool::new(false),
            async_command_txs,
            next_async_worker_index: AtomicUsize::new(0),
        }
    }

    /// Stops the runtime. Safe to call multiple times.
    #[cfg_attr(test, mutants::skip)] // Tests will hang if we mutate away the stop signal.
    pub fn stop(&self) {
        // If we've already started shutting down, don't do it again.
        if self.shutdown_started.fetch_or(true, Ordering::Relaxed) {
            return;
        }

        for (tx, waker, _domain) in &self.async_command_txs {
            // We ignore the result here because we do not care if the channel is already closed for
            // whatever reason (after all, that is relatively compatible with the "shut down" idea).
            _ = tx.send(AsyncWorkerCommand::<TS>::Shutdown);
            waker.notify();
        }
    }

    #[expect(
        clippy::arithmetic_side_effects,
        reason = "impossible to divide by zero due to NonEmpty"
    )]
    fn next_worker_index(&self) -> usize {
        self.next_async_worker_index.fetch_add(1, Ordering::Relaxed) % self.async_command_txs.len()
    }

    pub fn spawn<FF, F, R>(&self, placement: Placement, future_factory: FF) -> RemoteJoinHandle<R>
    where
        FF: FnOnce(TS, Domain) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        // For the moment, we implement only "Any". and "SameThreadAs" placements. This is just
        // a temporary lack of proper implementation. We accept all values to allow for API
        // experimentation - we can backfill it with correct functionality later.

        if self.shutdown_started.load(Ordering::Relaxed) {
            return RemoteJoinHandle::new_never();
        }

        let worker_index = if let Placement::SameThreadAs(placement_token) = placement {
            assert!(
                placement_token.domain.index() < self.async_command_txs.len(),
                "placement token contains invalid data - it may be from a different instance of the runtime"
            );

            placement_token.domain.index()
        } else {
            self.next_worker_index()
        };

        let (tx, waker, domain) = &self.async_command_txs
            .get(worker_index)
            .expect("next_worker_index() promises to give us only valid indexes and we asserted that placement token provided index fits in bounds");
        let domain = *domain;

        let (result_tx, result_rx) = once_event::shared::new_inefficient();

        // There is nothing we can really do if the worker is already gone and closed the channel.
        // That is a critical error that should result in a service restart to recover from.
        // TODO: We should have some high level design for handling unrecoverable errors like this.
        _ = tx.send(AsyncWorkerCommand::EnqueueTask {
            // The outer factory wraps the inner one and captures the result in addition to
            // what the inner does (merely executes some code).
            future_factory: Box::new(move |cx| {
                Box::pin(async move {
                    let inner = future_factory(cx, domain);
                    let result = inner.await;
                    result_tx.set(result);
                })
            }),
        });
        waker.notify();

        RemoteJoinHandle::new(result_rx, PlacementToken::new(domain))
    }

    pub fn spawn_multiple<FF, F, R>(
        &self,
        _placement: Placement,
        instantiation: Instantiation,
        future_factory: FF,
    ) -> Box<[RemoteJoinHandle<R>]>
    where
        FF: FnOnce(TS, SpawnInstance, Domain) -> F + Clone + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        // For the moment, we ignore placement and accept any value as meaning "Any". This is just
        // a temporary lack of proper implementation. We accept all values to allow for API
        // experimentation - we can backfill it with correct functionality later.

        assert_eq!(
            instantiation,
            Instantiation::All,
            "only All instantiation is supported"
        );

        if self.shutdown_started.load(Ordering::Relaxed) {
            return Box::new([]);
        }

        let count = self.async_command_txs.len();
        let mut join_handles = Vec::with_capacity(count);

        for (index, (tx, waker, domain)) in self.async_command_txs.iter().enumerate() {
            let domain = *domain;
            let instance_info = SpawnInstance::new(index, count);

            let (result_tx, result_rx) = once_event::shared::new_inefficient();

            // There is nothing we can really do if the worker is already gone and closed the channel.
            // That is a critical error that should result in a service restart to recover from.
            // TODO: We should have some high level design for handling unrecoverable errors like this.
            _ = tx.send(AsyncWorkerCommand::EnqueueTask {
                // The outer factory wraps the inner one to pass the instance info and captures the
                // result in addition to what the inner does (merely executes some code).
                future_factory: Box::new({
                    let future_factory = future_factory.clone();

                    move |cx| {
                        Box::pin(async move {
                            let inner = future_factory(cx, instance_info, domain);
                            let result = inner.await;
                            result_tx.set(result);
                        })
                    }
                }),
            });
            waker.notify();

            join_handles.push(RemoteJoinHandle::new(
                result_rx,
                PlacementToken::new(domain),
            ));
        }

        join_handles.into_boxed_slice()
    }
}

impl<WFS, TS> DispatcherCore<WFS, TS>
where
    WFS: WaitForShutdown,
    TS: RuntimeThreadState,
{
    /// Waits for the runtime to shut down and all worker threads to exit.
    ///
    /// Safe to call multiple times.
    pub fn join(&self) -> Pin<Box<dyn Future<Output = ()>>> {
        Box::pin(self.wait_for_shutdown.wait())
    }
}

#[cfg(test)]
mod tests {
    use isolated_domains::create_domains;
    use mpsc::TryRecvError;
    use oxidizer_testing::TEST_TIMEOUT;

    use super::*;
    use crate::dispatch::MockWaitForShutdown;
    use crate::{TestTaskContext, ThreadWaker};

    #[test]
    fn dispatch_spawn_single() {
        // We dispatch two tasks to the only worker.

        let (worker_tx, worker_rx) = mpsc::channel();
        let wfs = MockWaitForShutdown::new();
        let domains = create_domains(1);

        let dispatcher = DispatcherCore::<_, TestTaskContext>::new(
            wfs,
            NonEmpty::from_vec(vec![(worker_tx, ThreadWaker::new().into(), domains[0])]).unwrap(),
        );

        dispatcher.spawn(Placement::Any, |_, _| async move {
            unreachable!("we do not expect the task to be executed")
        });

        dispatcher.spawn(Placement::Any, |_, _| async move {
            unreachable!("we do not expect the task to be executed")
        });

        assert_eq!(drain_rx(&worker_rx).len(), 2);
    }

    #[test]
    fn dispatch_spawn_two() {
        // We dispatch two tasks to two workers, one each. Note that this test assumes we have
        // "fair" scheduling where tasks are evenly spread (e.g. round-robin). This is a coincidence
        // today as it is simply an implementation detail. If we change our scheduling logic, we
        // have to refactor this test to plug in a specific round-robin scheduling strategy to
        // accommodate the expectations (or perhaps move such a test to a test of the strategy).

        let (worker1_tx, worker1_rx) = mpsc::channel();
        let (worker2_tx, worker2_rx) = mpsc::channel();
        let wfs = MockWaitForShutdown::new();
        let domains = create_domains(2);

        let dispatcher = DispatcherCore::<_, TestTaskContext>::new(
            wfs,
            NonEmpty::from_vec(vec![
                (worker1_tx, ThreadWaker::new().into(), domains[0]),
                (worker2_tx, ThreadWaker::new().into(), domains[1]),
            ])
            .unwrap(),
        );

        dispatcher.spawn(Placement::Any, |_, _| async move {
            unreachable!("we do not expect the task to be executed")
        });

        dispatcher.spawn(Placement::Any, |_, _| async move {
            unreachable!("we do not expect the task to be executed")
        });

        assert_eq!(drain_rx(&worker1_rx).len(), 1);
        assert_eq!(drain_rx(&worker2_rx).len(), 1);
    }

    #[test]
    fn dispatch_spawn_multiple_all() {
        // We use n-ary dispatch to dispatch two tasks to two workers, one each. This does not make
        // any assumptions about the scheduling strategy - we always expect full worker coverage.

        let (worker1_tx, worker1_rx) = mpsc::channel();
        let (worker2_tx, worker2_rx) = mpsc::channel();
        let wfs = MockWaitForShutdown::new();
        let domains = create_domains(2);

        let dispatcher = DispatcherCore::<_, TestTaskContext>::new(
            wfs,
            NonEmpty::from_vec(vec![
                (worker1_tx, ThreadWaker::new().into(), domains[0]),
                (worker2_tx, ThreadWaker::new().into(), domains[1]),
            ])
            .unwrap(),
        );

        dispatcher.spawn_multiple(Placement::Any, Instantiation::All, |_, _, _| async move {
            unreachable!("we do not expect the task to be executed")
        });

        assert_eq!(drain_rx(&worker1_rx).len(), 1);
        assert_eq!(drain_rx(&worker2_rx).len(), 1);
    }

    #[test]
    fn dispatcher_stop_sends_one_stop_command_to_each_worker() {
        let (worker1_tx, worker1_rx) = mpsc::channel();
        let (worker2_tx, worker2_rx) = mpsc::channel();
        let wfs = MockWaitForShutdown::new();
        let domains = create_domains(2);

        let dispatcher = DispatcherCore::<_, TestTaskContext>::new(
            wfs,
            NonEmpty::from_vec(vec![
                (worker1_tx, ThreadWaker::new().into(), domains[0]),
                (worker2_tx, ThreadWaker::new().into(), domains[1]),
            ])
            .unwrap(),
        );

        assert_eq!(drain_rx(&worker1_rx).len(), 0);
        assert_eq!(drain_rx(&worker2_rx).len(), 0);

        dispatcher.stop();

        assert_eq!(drain_rx(&worker1_rx).len(), 1);
        assert_eq!(drain_rx(&worker2_rx).len(), 1);

        // Call for a stop twice - we expect no further stop commands to be sent.
        dispatcher.stop();

        assert_eq!(drain_rx(&worker1_rx).len(), 0);
        assert_eq!(drain_rx(&worker2_rx).len(), 0);
    }

    #[test]
    fn no_dispatch_after_stop() {
        // Once the stop command has been given, we expect dispatched tasks to be ignored & dropped
        // because there is no longer any guarantee that anyone is listening for them.

        let (worker_tx, worker_rx) = mpsc::channel();
        let wfs = MockWaitForShutdown::new();
        let domains = create_domains(1);

        let dispatcher = DispatcherCore::<_, TestTaskContext>::new(
            wfs,
            NonEmpty::from_vec(vec![(worker_tx, ThreadWaker::new().into(), domains[0])]).unwrap(),
        );

        dispatcher.stop();

        // Drain the shutdown command.
        _ = worker_rx.recv_timeout(TEST_TIMEOUT).unwrap();

        dispatcher.spawn(Placement::Any, |_, _| async move {});
        assert_eq!(drain_rx(&worker_rx).len(), 0);

        dispatcher.spawn_multiple(Placement::Any, Instantiation::All, |_, _, _| async move {});
        assert_eq!(drain_rx(&worker_rx).len(), 0);
    }

    fn drain_rx<TS>(
        worker_rx: &mpsc::Receiver<AsyncWorkerCommand<TS>>,
    ) -> Vec<AsyncWorkerCommand<TS>>
    where
        TS: RuntimeThreadState,
    {
        let mut commands = Vec::new();

        loop {
            match worker_rx.try_recv() {
                Ok(command) => commands.push(command),
                Err(TryRecvError::Empty) => return commands,
                Err(TryRecvError::Disconnected) => panic!("worker channel disconnected"),
            }
        }
    }
}