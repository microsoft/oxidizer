// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::{Any, type_name};
use std::cell::{Cell, RefCell};
use std::fmt;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::task::{Wake, Waker};
use std::time::Duration;

use arty_io_core::{Cycle, Driver, DriverError, DriverOptions, DriverRole, IoContext};
use observed::{Sink, emit};
use thread_aware_core::Thread;

use crate::IoRuntime;
use crate::runtime::Resolution;
use crate::telemetry::{
    IoContextName, IoDriverCycleFailed, IoDriverErrorMessage, IoDriverInitializationFailed, IoDriverInitialized, IoDriverRoleLabel,
    IoDriverShutdownFailed,
};

trait ErasedDriver {
    fn execute_cycle(&mut self, cycle: &mut Cycle) -> Result<(), DriverError>;
    fn shutdown(self: Box<Self>) -> Result<(), DriverError>;
}

impl<D> ErasedDriver for D
where
    D: Driver,
{
    fn execute_cycle(&mut self, cycle: &mut Cycle) -> Result<(), DriverError> {
        Driver::execute_cycle(self, cycle)
    }

    fn shutdown(self: Box<Self>) -> Result<(), DriverError> {
        Driver::shutdown(*self)
    }
}

struct LocalContext {
    context: Box<dyn Any>,
    driver: Box<dyn ErasedDriver>,
    role: DriverRole,
    context_name: &'static str,
}

struct InitializationGuard<'a>(&'a Cell<bool>);

impl Drop for InitializationGuard<'_> {
    fn drop(&mut self) {
        assert!(self.0.replace(false), "worker-local I/O initialization guard must be active");
    }
}

#[derive(Default)]
struct DriverInterrupt {
    primary: Mutex<Option<Arc<Waker>>>,
}

impl DriverInterrupt {
    fn set(&self, waker: Waker) {
        let mut primary = self
            .primary
            .lock()
            .expect("a poisoned driver interrupt indicates a runtime invariant failure");
        assert!(
            primary.replace(Arc::new(waker)).is_none(),
            "a worker has at most one primary I/O driver"
        );
    }

    fn clear(&self) {
        let waker = self
            .primary
            .lock()
            .expect("a poisoned driver interrupt indicates a runtime invariant failure")
            .take();
        drop(waker);
    }

    fn wake_primary(&self) {
        let waker = self
            .primary
            .lock()
            .expect("a poisoned driver interrupt indicates a runtime invariant failure")
            .clone();
        if let Some(waker) = waker {
            waker.wake_by_ref();
        }
    }
}

impl Wake for DriverInterrupt {
    fn wake(self: Arc<Self>) {
        self.wake_primary();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.wake_primary();
    }
}

/// Worker-local I/O driver host used by the Arty runtime.
///
/// This type is unstable Arty implementation infrastructure. Applications and
/// libraries should use `arty` instead.
///
/// Dropping the host clears its interrupt target, then attempts to shut down
/// every secondary driver followed by the primary. Explicit shutdown errors are
/// emitted through the configured sink and do not skip remaining drivers.
/// Panics from drivers or diagnostics propagate.
pub struct DriverHost {
    runtime: IoRuntime,
    thread: Thread,
    owner_waker: Waker,
    sink: Sink,
    interrupt: Arc<DriverInterrupt>,
    primary: RefCell<Option<LocalContext>>,
    secondaries: RefCell<Vec<LocalContext>>,
    initializing: Cell<bool>,
    _single_threaded: PhantomData<Rc<()>>,
}

impl fmt::Debug for DriverHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DriverHost").field("thread", &self.thread).finish_non_exhaustive()
    }
}

impl DriverHost {
    pub(crate) fn new(runtime: IoRuntime, thread: Thread, owner_waker: Waker, sink: Sink) -> Self {
        Self {
            runtime,
            thread,
            owner_waker,
            sink,
            interrupt: Arc::new(DriverInterrupt::default()),
            primary: RefCell::new(None),
            secondaries: RefCell::new(Vec::new()),
            initializing: Cell::new(false),
            _single_threaded: PhantomData,
        }
    }

    /// Returns a waker that interrupts a waiting primary driver.
    ///
    /// Clones remain safe after the host is dropped and become inert.
    #[must_use]
    pub fn waker(&self) -> Waker {
        Waker::from(Arc::clone(&self.interrupt))
    }

    /// Returns this worker's context of type `C`, creating its local driver on
    /// first access.
    ///
    /// # Errors
    ///
    /// Returns a driver error when nested context access is attempted or driver
    /// construction fails.
    ///
    /// # Panics
    ///
    /// Propagates panics from context construction, cloning, relocation, driver
    /// construction, custom wakers, diagnostics, and violated host invariants.
    pub fn context<C>(&self, options: &C::ContextOptions) -> Result<C, DriverError>
    where
        C: IoContext,
        C::ContextOptions: Clone,
    {
        if let Some(context) = self
            .primary
            .borrow()
            .as_ref()
            .and_then(Self::try_clone_context::<C>)
            .or_else(|| self.secondaries.borrow().iter().find_map(Self::try_clone_context::<C>))
        {
            return Ok(context);
        }
        if self.initializing.replace(true) {
            return Err(DriverError::from_message(
                "I/O context retrieval cannot be nested during initialization",
            ));
        }
        let _initialization = InitializationGuard(&self.initializing);
        let Resolution { mut context, role } = self.runtime.get_context::<C>(options);
        context.relocate(None, &self.thread);
        let local = self.create_local_driver::<C>(context, role)?;
        let context = Self::clone_context::<C>(&local);
        match local.role {
            DriverRole::Primary => {
                assert!(
                    self.primary.borrow_mut().replace(local).is_none(),
                    "a worker has at most one primary I/O driver"
                );
            }
            DriverRole::Secondary => self.secondaries.borrow_mut().push(local),
        }
        Ok(context)
    }

    fn create_local_driver<C>(&self, mut context: C, role: DriverRole) -> Result<LocalContext, DriverError>
    where
        C: IoContext,
    {
        let options = DriverOptions::new(self.thread.clone(), self.owner_waker.clone(), role);
        let driver = context.driver(options).inspect_err(|error| {
            emit!(
                &self.sink,
                IoDriverInitializationFailed {
                    context: IoContextName(type_name::<C>()),
                    role: IoDriverRoleLabel::from(role),
                    error: IoDriverErrorMessage::new(error),
                }
            );
        })?;
        if role == DriverRole::Primary {
            self.interrupt.set(driver.waker());
        }
        emit!(
            &self.sink,
            IoDriverInitialized {
                context: IoContextName(type_name::<C>()),
                role: IoDriverRoleLabel::from(role),
            }
        );
        Ok(LocalContext {
            context: Box::new(context),
            driver: Box::new(driver),
            role,
            context_name: type_name::<C>(),
        })
    }

    fn clone_context<C>(local: &LocalContext) -> C
    where
        C: IoContext,
    {
        local
            .context
            .downcast_ref::<C>()
            .expect("worker-local I/O contexts are stored under their own TypeId")
            .clone()
    }

    fn try_clone_context<C>(local: &LocalContext) -> Option<C>
    where
        C: IoContext,
    {
        local.context.downcast_ref::<C>().cloned()
    }

    /// Executes one driver cycle, running secondary drivers before the primary.
    ///
    /// # Errors
    ///
    /// Returns the first driver cycle failure after emitting its diagnostic.
    ///
    /// # Panics
    ///
    /// Propagates panics from drivers, diagnostics, and violated host
    /// invariants.
    pub fn execute_cycle(&self, max_wait: Duration) -> Result<(), DriverError> {
        for local in self.secondaries.borrow_mut().iter_mut() {
            Self::execute_driver(local, Duration::ZERO, &self.sink)?;
        }
        if let Some(primary) = self.primary.borrow_mut().as_mut() {
            Self::execute_driver(primary, max_wait, &self.sink)?;
        }
        Ok(())
    }

    fn execute_driver(local: &mut LocalContext, max_wait: Duration, sink: &Sink) -> Result<(), DriverError> {
        local.driver.execute_cycle(&mut Cycle::new(max_wait)).inspect_err(|error| {
            emit!(
                sink,
                IoDriverCycleFailed {
                    context: IoContextName(local.context_name),
                    role: IoDriverRoleLabel::from(local.role),
                    error: IoDriverErrorMessage::new(error),
                }
            );
        })
    }
}

impl Drop for DriverHost {
    fn drop(&mut self) {
        self.interrupt.clear();

        let primary = self.primary.get_mut().take();
        let secondaries = std::mem::take(self.secondaries.get_mut());
        for local in secondaries.into_iter().chain(primary) {
            if let Err(error) = local.driver.shutdown() {
                emit!(
                    &self.sink,
                    IoDriverShutdownFailed {
                        context: IoContextName(local.context_name),
                        role: IoDriverRoleLabel::from(local.role),
                        error: IoDriverErrorMessage::new(&error),
                    }
                );
            }
        }
    }
}
