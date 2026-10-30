// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::future::pending;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use arty::core::{Thread, ThreadAware};
use arty::task::{Builtins, JoinHandle};
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

pub(crate) async fn initialize_all_workers(cx: &Builtins, tokio: TokioHandle) -> Result<(), InitError> {
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
    let mut abort_on_drop = AbortOnDrop::new(Arc::clone(&transaction));

    let mut failure = None;
    let mut ready = 0;
    for installer in &mut installers {
        match installer.wait_until_started().await {
            Ok(Some(StartupResult::Ready)) => ready += 1,
            Ok(Some(StartupResult::ContextConflict)) => {
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

    if commit_if_started(&transaction, startup_succeeded(failure.as_ref(), ready, installers.len())) {
        abort_on_drop.disarm();
        return Ok(());
    }

    transaction.abort();
    finish_installers(&mut installers, &mut failure).await;
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
        abort_on_drop.disarm();
        return;
    }
    if Handle::try_current().is_ok() {
        data.transaction.abort();
        let _send_result = data.startup.send(StartupResult::ContextConflict);
        abort_on_drop.disarm();
        return;
    }

    let _entered = data.transaction.handle.enter();
    let _send_result = data.startup.send(StartupResult::Ready);

    loop {
        match data.transaction.phase() {
            Phase::Preparing => {
                data.phase_changed
                    .changed_async()
                    .await
                    .expect("each installer retains the transaction phase sender");
            }
            Phase::Committed => {
                abort_on_drop.disarm();
                pending::<()>().await;
            }
            Phase::Aborted => {
                abort_on_drop.disarm();
                return;
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::future::pending;
    use std::sync::Arc;

    use arty::runtime::{Runtime as ArtyRuntime, WorkersPolicy};
    use performables::sync::channel::oneshot;
    use tokio::runtime::{Builder, Runtime};

    use super::{AbortOnDrop, Installer, Phase, StartupResult, Transaction, commit_if_started, finish_installers, startup_succeeded};

    fn transaction() -> (Arc<Transaction>, Runtime) {
        let runtime = Builder::new_multi_thread().enable_all().build().unwrap();
        let handle = runtime.handle().clone();
        (Transaction::new(handle).0, runtime)
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

        assert!(failure.is_some());
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
