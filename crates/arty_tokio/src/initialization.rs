// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::future::pending;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
#[cfg(test)]
use std::sync::{Mutex, mpsc};

use arty::core::{Thread, ThreadAware};
use arty::task::{Builtins, JoinHandle};
use performables::sync::channel::{OneshotReceiver, OneshotSender, WatchReceiver, WatchSender, oneshot, watch};
use tokio::runtime::{Handle, Runtime, RuntimeFlavor};

use crate::error::{classify_join, context_conflict, initialization_aborted, runtime_build, unsupported_runtime};
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

enum RuntimeOwner {
    Internal(Runtime),
    External(Handle),
    #[cfg(test)]
    Test(TestRuntimeOwner),
}

impl RuntimeOwner {
    fn new(tokio: TokioHandle) -> Result<Self, InitError> {
        match tokio.kind {
            crate::TokioHandleKind::Default => {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .map_err(runtime_build)?;
                Ok(Self::Internal(runtime))
            }
            crate::TokioHandleKind::External(handle) => {
                if !matches!(handle.runtime_flavor(), RuntimeFlavor::MultiThread) {
                    return Err(unsupported_runtime());
                }
                Ok(Self::External(handle))
            }
        }
    }

    fn handle(&self) -> Handle {
        match self {
            Self::Internal(runtime) => runtime.handle().clone(),
            Self::External(handle) => handle.clone(),
            #[cfg(test)]
            Self::Test(owner) => owner.runtime.handle().clone(),
        }
    }
}

#[cfg(test)]
struct TestRuntimeOwner {
    runtime: Runtime,
    entered_drop: mpsc::SyncSender<()>,
    release_drop: Mutex<mpsc::Receiver<()>>,
}

#[cfg(test)]
impl Drop for TestRuntimeOwner {
    fn drop(&mut self) {
        assert!(
            self.entered_drop.send(()).is_ok(),
            "the shutdown test must observe the runtime owner destructor"
        );
        let release_drop = self.release_drop.get_mut().unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(
            release_drop.recv().is_ok(),
            "the shutdown test must release the runtime owner destructor"
        );
    }
}

struct Transaction {
    phase: AtomicU8,
    phase_changed: WatchSender<()>,
    runtime: RuntimeOwner,
}

impl Transaction {
    fn new(runtime: RuntimeOwner) -> (Arc<Self>, WatchReceiver<()>) {
        let (phase_changed, receiver) = watch(());
        (
            Arc::new(Self {
                phase: AtomicU8::new(PHASE_PREPARING),
                phase_changed,
                runtime,
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
        let Some(join) = self.join.take() else {
            return Ok(None);
        };
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
    let runtime = RuntimeOwner::new(tokio)?;
    initialize_all_workers_with_runtime(cx, runtime).await
}

async fn initialize_all_workers_with_runtime(cx: &Builtins, runtime: RuntimeOwner) -> Result<(), InitError> {
    let workers: Vec<Thread> = cx.control().workers().into_iter().cloned().collect();
    let (transaction, phase_changed) = Transaction::new(runtime);
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

    if failure.is_none() && ready == installers.len() && transaction.commit() {
        abort_on_drop.disarm();
        return Ok(());
    }

    transaction.abort();
    for installer in &mut installers {
        if let Err(error) = installer.finish().await {
            failure.get_or_insert(error);
        }
    }
    abort_on_drop.disarm();
    Err(failure.unwrap_or_else(initialization_aborted))
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

    let handle = data.transaction.runtime.handle();
    let _entered = handle.enter();
    if data.startup.send(StartupResult::Ready).is_err() {
        return;
    }

    loop {
        match data.transaction.phase() {
            Phase::Preparing => {
                if data.phase_changed.changed_async().await.is_err() {
                    return;
                }
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
    use std::sync::mpsc;
    use std::time::Duration;

    use arty::runtime::{Runtime as ArtyRuntime, WorkersPolicy};
    use tokio::runtime::Builder;

    use super::{RuntimeOwner, TestRuntimeOwner, initialize_all_workers_with_runtime};

    const TEST_TIMEOUT: Duration = Duration::from_secs(10);

    // The public lifecycle regression is in tests/lifecycle.rs. This unit test
    // injects the private owner-drop seam, which the public API cannot provide.
    #[test]
    fn arty_shutdown_waits_for_runtime_owner_destructor() {
        let runtime = ArtyRuntime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
        let tokio = Builder::new_multi_thread().enable_all().build().unwrap();
        let (entered_drop, drop_entered) = mpsc::sync_channel(1);
        let (release_drop, drop_release) = mpsc::sync_channel(1);
        let owner = RuntimeOwner::Test(TestRuntimeOwner {
            runtime: tokio,
            entered_drop,
            release_drop: std::sync::Mutex::new(drop_release),
        });

        runtime
            .scheduler()
            .block_on(async move |cx| {
                initialize_all_workers_with_runtime(&cx, owner).await.unwrap();
            })
            .unwrap();

        let (stopped, completion) = mpsc::sync_channel(1);
        let shutdown = std::thread::spawn(move || {
            stopped.send(runtime.stop()).unwrap();
        });

        drop_entered.recv_timeout(TEST_TIMEOUT).unwrap();
        assert!(matches!(completion.try_recv(), Err(mpsc::TryRecvError::Empty)));

        release_drop.send(()).unwrap();
        completion.recv_timeout(TEST_TIMEOUT).unwrap().unwrap();
        shutdown.join().unwrap();
    }
}
