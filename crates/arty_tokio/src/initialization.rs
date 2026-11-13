// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::future::pending;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use arty::core::{Thread, ThreadAware};
use arty::task::{Context, JoinHandle};
use performables::sync::channel::{OneshotReceiver, OneshotSender, WatchReceiver, WatchSender, oneshot, watch};
use tokio::runtime::{Handle, RuntimeFlavor};

use crate::error::{classify_join, context_conflict, initialization_aborted, unsupported_runtime};
use crate::{InitError, TokioHandle};

const PHASE_PREPARING: u8 = 0;
const PHASE_COMMITTED: u8 = 1;
const PHASE_ABORTED: u8 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Preparing,
    Committed,
    Aborted,
}

impl Phase {
    const fn from_raw(value: u8) -> Self {
        match value {
            PHASE_COMMITTED => Self::Committed,
            PHASE_ABORTED => Self::Aborted,
            _ => Self::Preparing,
        }
    }
}

struct Transaction {
    phase: AtomicU8,
    phase_changed: WatchSender<()>,
    handle: Handle,
}

impl Transaction {
    fn new(handle: Handle) -> (Arc<Self>, WatchReceiver<()>) {
        let (phase_changed, receiver) = watch(());
        (
            Arc::new(Self {
                phase: AtomicU8::new(PHASE_PREPARING),
                phase_changed,
                handle,
            }),
            receiver,
        )
    }

    fn phase(&self) -> Phase {
        Phase::from_raw(self.phase.load(Ordering::Acquire))
    }

    fn commit(&self) -> bool {
        self.transition(PHASE_COMMITTED)
    }

    fn abort(&self) {
        self.transition(PHASE_ABORTED);
    }

    fn transition(&self, destination: u8) -> bool {
        if self
            .phase
            .compare_exchange(PHASE_PREPARING, destination, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        self.phase_changed.send_replace(());
        true
    }
}

struct AbortOnDrop {
    transaction: Arc<Transaction>,
    armed: bool,
}

impl AbortOnDrop {
    fn new(transaction: Arc<Transaction>) -> Self {
        Self { transaction, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.transaction.abort();
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StartupResult {
    Ready,
    ContextConflict,
}

struct InstallerData {
    transaction: Arc<Transaction>,
    phase_changed: WatchReceiver<()>,
    startup: OneshotSender<StartupResult>,
}

impl ThreadAware for InstallerData {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

struct Installer {
    startup: OneshotReceiver<StartupResult>,
    join: Option<JoinHandle<()>>,
}

impl Installer {
    async fn wait_until_started(&mut self) -> Result<Option<StartupResult>, InitError> {
        if let Ok(result) = (&mut self.startup).await {
            return Ok(Some(result));
        }
        let join = self.join.take().expect("each installer waits for startup at most once");
        match join.await {
            Ok(()) => Ok(None),
            Err(error) => Err(classify_join(error)),
        }
    }

    async fn finish(&mut self) -> Result<(), InitError> {
        let Some(join) = self.join.take() else {
            return Ok(());
        };
        join.await.map_err(classify_join)
    }
}

pub(crate) async fn initialize_all_workers(cx: &Context, tokio: TokioHandle) -> Result<(), InitError> {
    let handle = tokio.handle;
    if !matches!(handle.runtime_flavor(), RuntimeFlavor::MultiThread) {
        return Err(unsupported_runtime());
    }
    let workers: Vec<Thread> = cx.control().workers().into_iter().cloned().collect();
    let (transaction, phase_changed) = Transaction::new(handle);
    let mut installers = workers
        .iter()
        .map(|worker| {
            let (startup, startup_result) = oneshot();
            let data = InstallerData {
                transaction: Arc::clone(&transaction),
                phase_changed: phase_changed.clone(),
                startup,
            };
            Installer {
                startup: startup_result,
                join: Some(cx.scheduler().spawn_at(worker, data, install_worker)),
            }
        })
        .collect::<Vec<_>>();
    coordinate_installers(&transaction, &mut installers, Installer::wait_until_started, finish_installers).await
}

async fn coordinate_installers<I>(
    transaction: &Arc<Transaction>,
    installers: &mut [I],
    mut wait_until_started: impl AsyncFnMut(&mut I) -> Result<Option<StartupResult>, InitError>,
    finish: impl AsyncFnOnce(&mut [I], &mut Option<InitError>),
) -> Result<(), InitError> {
    let mut abort_on_drop = AbortOnDrop::new(Arc::clone(transaction));

    let mut failure = None;
    let mut ready = 0;
    for installer in installers.iter_mut() {
        match wait_until_started(installer).await {
            Ok(Some(StartupResult::Ready)) => ready += 1,
            Ok(Some(StartupResult::ContextConflict)) => {
                // #[gamma::skip(stmt.delete_call, reason = "install_worker aborts the transaction before sending ContextConflict.")]
                transaction.abort();
                failure.get_or_insert_with(context_conflict);
            }
            Ok(None) => {}
            Err(error) => {
                transaction.abort();
                failure.get_or_insert(error);
            }
        }
    }

    if
    // #[gamma::skip(call_result.default, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
    commit_if_started(transaction, startup_succeeded(failure.as_ref(), ready, installers.len())) {
        // #[gamma::skip(stmt.delete_call, reason = "The successful commit already made the transaction terminal; another abort cannot change it.")]
        abort_on_drop.disarm();
        return Ok(());
    }

    transaction.abort();
    finish(installers, &mut failure).await;
    // #[gamma::skip(stmt.delete_call, reason = "The transaction is already aborted; the drop guard's abort cannot change a terminal phase.")]
    abort_on_drop.disarm();
    Err(failure.unwrap_or_else(initialization_aborted))
}

async fn finish_installers(installers: &mut [Installer], failure: &mut Option<InitError>) {
    for installer in installers {
        if let Err(error) = installer.finish().await {
            failure.get_or_insert(error);
        }
    }
}

fn startup_succeeded(failure: Option<&InitError>, ready: usize, expected: usize) -> bool {
    if failure.is_some() {
        return false;
    }
    ready == expected
}

fn commit_if_started(transaction: &Transaction, started: bool) -> bool {
    if !started {
        return false;
    }
    transaction.commit()
}

#[expect(clippy::future_not_send, reason = "the fixed-worker task must retain Tokio's non-Send EnterGuard")]
async fn install_worker(data: InstallerData) {
    let mut abort_on_drop = AbortOnDrop::new(Arc::clone(&data.transaction));
    if data.transaction.phase() != Phase::Preparing {
        // #[gamma::skip(stmt.delete_call, reason = "A non-preparing transaction is terminal, so dropping an armed guard cannot change its phase.")]
        abort_on_drop.disarm();
        return;
    }
    if Handle::try_current().is_ok() {
        data.transaction.abort();
        let _send_result = data.startup.send(StartupResult::ContextConflict);
        // #[gamma::skip(stmt.delete_call, reason = "The context-conflict branch already aborted the transaction.")]
        abort_on_drop.disarm();
        return;
    }

    let _entered = data.transaction.handle.enter();
    let _send_result = data.startup.send(StartupResult::Ready);

    loop {
        match data.transaction.phase() {
            Phase::Preparing => {
                // #[gamma::skip(stmt.delete_call, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
                data.phase_changed
                    .changed_async()
                    .await
                    .expect("each installer retains the transaction phase sender");
            }
            Phase::Committed => {
                // #[gamma::skip(stmt.delete_call, reason = "Committed is terminal; abort on later cancellation cannot change it.")]
                abort_on_drop.disarm();
                pending::<()>().await;
            }
            Phase::Aborted => {
                // #[gamma::skip(stmt.delete_call, reason = "Aborted is terminal; aborting again on drop has no effect.")]
                abort_on_drop.disarm();
                return;
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #[test]
    fn gamma_round2_coordinator_publishes_abort_before_next_startup_and_before_finishing() {
        use std::cell::Cell;

        for startup_error in [false, true] {
            let runtime = Builder::new_current_thread().build().unwrap();
            let (transaction, observer) = Transaction::new(runtime.handle().clone());
            let mut installers = [0, 1];
            let finished = Cell::new(0);
            let result = futures::executor::block_on(super::coordinate_installers(
                &transaction,
                &mut installers,
                async |index: &mut i32| {
                    if *index == 0 && startup_error {
                        Err(crate::error::initialization_aborted())
                    } else {
                        if startup_error {
                            assert_eq!(transaction.phase(), Phase::Aborted);
                            assert!(observer.has_changed().unwrap());
                        }
                        Ok(None)
                    }
                },
                async |_: &mut [i32], failure: &mut Option<crate::InitError>| {
                    assert_eq!(transaction.phase(), Phase::Aborted);
                    assert!(observer.has_changed().unwrap());
                    assert_eq!(failure.is_some(), startup_error);
                    finished.set(finished.get() + 1);
                },
            ));
            assert!(result.is_err());
            assert_eq!(finished.get(), 1);
            assert_eq!(transaction.phase(), Phase::Aborted);
        }
    }

    use std::future::pending;
    use std::sync::Arc;

    use arty::runtime::{Runtime as ArtyRuntime, WorkersPolicy};
    use performables::sync::channel::oneshot;
    use tokio::runtime::{Builder, Runtime};

    use super::{
        AbortOnDrop, Installer, InstallerData, Phase, StartupResult, Transaction, commit_if_started, coordinate_installers,
        finish_installers, install_worker, startup_succeeded,
    };

    #[test]
    fn gamma_survivor_worker_preparing_cancellation_aborts_but_terminal_start_sends_no_ready() {
        use std::future::Future;
        use std::task::{Context, Waker};

        let runtime = Builder::new_current_thread().build().unwrap();
        let (transaction, phase_changed) = Transaction::new(runtime.handle().clone());
        let (startup, startup_result) = oneshot();
        let mut worker = Box::pin(super::install_worker(super::InstallerData {
            transaction: Arc::clone(&transaction),
            phase_changed,
            startup,
        }));
        assert!(worker.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
        assert!(matches!(futures::executor::block_on(startup_result), Ok(StartupResult::Ready)));
        assert_eq!(transaction.phase(), Phase::Preparing);
        drop(worker);
        assert_eq!(transaction.phase(), Phase::Aborted);
        tokio::runtime::Handle::try_current().unwrap_err();

        let (transaction, phase_changed) = Transaction::new(runtime.handle().clone());
        transaction.abort();
        let (startup, startup_result) = oneshot();
        let mut worker = Box::pin(super::install_worker(super::InstallerData {
            transaction: Arc::clone(&transaction),
            phase_changed,
            startup,
        }));
        assert!(worker.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_ready());
        futures::executor::block_on(startup_result).unwrap_err();
        assert_eq!(transaction.phase(), Phase::Aborted);
    }

    #[test]
    fn gamma_survivor_worker_context_conflict_aborts_and_reports_exact_startup_result() {
        use std::future::Future;
        use std::task::{Context, Waker};

        let runtime = Builder::new_current_thread().build().unwrap();
        let _entered = runtime.enter();
        let (transaction, phase_changed) = Transaction::new(runtime.handle().clone());
        let (startup, startup_result) = oneshot();
        let mut worker = Box::pin(super::install_worker(super::InstallerData {
            transaction: Arc::clone(&transaction),
            phase_changed,
            startup,
        }));
        assert!(worker.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_ready());
        assert!(matches!(
            futures::executor::block_on(startup_result),
            Ok(StartupResult::ContextConflict)
        ));
        assert_eq!(transaction.phase(), Phase::Aborted);
    }

    fn transaction() -> (Arc<Transaction>, Runtime) {
        let runtime = Builder::new_multi_thread().enable_all().build().unwrap();
        let handle = runtime.handle().clone();
        (Transaction::new(handle).0, runtime)
    }

    #[test]
    fn gamma_survivor_transaction_has_one_terminal_transition_and_one_notification() {
        let runtime = Builder::new_current_thread().build().unwrap();
        for commit in [false, true] {
            let (transaction, receiver) = Transaction::new(runtime.handle().clone());
            assert_eq!(transaction.phase(), Phase::Preparing);
            if commit {
                assert!(transaction.commit());
            } else {
                transaction.abort();
            }
            assert_eq!(receiver.changed(), Ok(()));
            let terminal = transaction.phase();
            assert!(!transaction.commit());
            transaction.abort();
            assert_eq!(transaction.phase(), terminal);
            let mut changed = std::pin::pin!(receiver.changed_async());
            assert!(
                changed
                    .as_mut()
                    .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
                    .is_pending()
            );
        }
    }

    #[test]
    fn raw_phase_values_decode_to_their_states() {
        assert_eq!(
            [
                Phase::from_raw(super::PHASE_PREPARING),
                Phase::from_raw(super::PHASE_COMMITTED),
                Phase::from_raw(super::PHASE_ABORTED),
            ],
            [Phase::Preparing, Phase::Committed, Phase::Aborted]
        );
    }

    #[test]
    fn transaction_commit_changes_the_phase() {
        let (transaction, _runtime) = transaction();

        assert!(transaction.commit());
        assert_eq!(transaction.phase(), Phase::Committed);
    }

    #[test]
    fn transaction_abort_changes_the_phase() {
        let (transaction, _runtime) = transaction();

        transaction.abort();

        assert_eq!(transaction.phase(), Phase::Aborted);
    }

    #[test]
    fn armed_guard_aborts_the_transaction() {
        let (transaction, _runtime) = transaction();

        drop(AbortOnDrop::new(Arc::clone(&transaction)));

        assert_eq!(transaction.phase(), Phase::Aborted);
    }

    #[test]
    fn disarmed_guard_preserves_the_transaction() {
        let (transaction, _runtime) = transaction();
        let mut guard = AbortOnDrop::new(Arc::clone(&transaction));

        guard.disarm();
        drop(guard);

        assert_eq!(transaction.phase(), Phase::Preparing);
    }

    #[test]
    fn installer_reports_the_startup_result() {
        let (startup, startup_result) = oneshot();
        startup.send(StartupResult::Ready).unwrap();
        let mut installer = Installer {
            startup: startup_result,
            join: None,
        };

        let result = futures::executor::block_on(installer.wait_until_started()).unwrap();

        assert_eq!(result, Some(StartupResult::Ready));
    }

    #[test]
    fn installer_that_exits_without_readiness_aborts_the_transaction() {
        let runtime = ArtyRuntime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
        let join = runtime.scheduler().block_on(async |cx| cx.scheduler().spawn(async |_| {})).unwrap();
        let (startup, startup_result) = oneshot();
        drop(startup);
        let mut installers = [Installer {
            startup: startup_result,
            join: Some(join),
        }];
        let (transaction, _tokio) = transaction();

        let result = futures::executor::block_on(coordinate_installers(
            &transaction,
            &mut installers,
            Installer::wait_until_started,
            finish_installers,
        ));
        runtime.stop().unwrap();

        assert_eq!(
            result.unwrap_err().to_string(),
            "Tokio compatibility initialization stopped before commit"
        );
        assert_eq!(transaction.phase(), Phase::Aborted);
        assert!(installers[0].join.is_none());
    }

    #[test]
    fn worker_does_not_enter_tokio_after_the_transaction_is_aborted() {
        let runtime = Builder::new_multi_thread().enable_all().build().unwrap();
        let (transaction, phase_changed) = Transaction::new(runtime.handle().clone());
        transaction.abort();
        let (startup, startup_result) = oneshot();
        let data = InstallerData {
            transaction: Arc::clone(&transaction),
            phase_changed,
            startup,
        };

        futures::executor::block_on(install_worker(data));

        futures::executor::block_on(startup_result).unwrap_err();
        assert_eq!(transaction.phase(), Phase::Aborted);
        tokio::runtime::Handle::try_current().unwrap_err();
    }

    #[test]
    fn installer_reports_successful_exit_without_startup() {
        let runtime = ArtyRuntime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
        let join = runtime.scheduler().block_on(async |cx| cx.scheduler().spawn(async |_| {})).unwrap();
        let (startup, startup_result) = oneshot();
        drop(startup);
        let mut installer = Installer {
            startup: startup_result,
            join: Some(join),
        };

        let result = futures::executor::block_on(installer.wait_until_started()).unwrap();

        assert_eq!(result, None);
        assert!(installer.join.is_none());
        runtime.stop().unwrap();
    }

    #[test]
    fn installer_finish_reports_join_failure() {
        let runtime = ArtyRuntime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
        let join = runtime
            .scheduler()
            .block_on(async |cx| cx.scheduler().spawn(async |_| panic!("expected installer task failure")))
            .unwrap();
        let (_startup, startup_result) = oneshot();
        let mut installer = Installer {
            startup: startup_result,
            join: Some(join),
        };

        let result = futures::executor::block_on(installer.finish());
        runtime.stop().unwrap_err();

        assert!(result.is_err());
    }

    #[test]
    fn rollback_collects_join_failures_and_finishes_every_installer() {
        let runtime = ArtyRuntime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
        let (failed, canceled) = runtime
            .scheduler()
            .block_on(async |cx| {
                let failed = cx.scheduler().spawn(async |_| panic!("expected installer task failure"));
                let canceled = cx.scheduler().spawn(async |_| pending::<()>().await);
                (failed, canceled)
            })
            .unwrap();
        let (_first_startup, first_startup_result) = oneshot();
        let (_second_startup, second_startup_result) = oneshot();
        let mut installers = [
            Installer {
                startup: first_startup_result,
                join: Some(failed),
            },
            Installer {
                startup: second_startup_result,
                join: Some(canceled),
            },
        ];
        let mut failure = None;

        futures::executor::block_on(finish_installers(&mut installers, &mut failure));

        assert_eq!(failure.unwrap().to_string(), "a Tokio installer panicked\ncaused by: task panicked");
        assert!(installers.iter().all(|installer| installer.join.is_none()));
        runtime.stop().unwrap_err();
    }

    #[test]
    fn startup_requires_every_installer_and_no_failure() {
        let failure = super::initialization_aborted();

        assert!(startup_succeeded(None, 2, 2));
        assert!(!startup_succeeded(None, 1, 2));
        assert!(!startup_succeeded(Some(&failure), 2, 2));
    }

    #[test]
    fn commit_requires_successful_startup() {
        let (transaction, _runtime) = transaction();

        assert!(!commit_if_started(&transaction, false));
        assert_eq!(transaction.phase(), Phase::Preparing);
        assert!(commit_if_started(&transaction, true));
        assert_eq!(transaction.phase(), Phase::Committed);
    }
}
