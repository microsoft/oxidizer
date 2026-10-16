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

impl LocalContext {
    fn clone_context<C>(&self) -> C
    where
        C: IoContext,
    {
        self.context
            .downcast_ref::<C>()
            .expect("worker-local I/O contexts are stored under their own TypeId")
            .clone()
    }

    fn try_clone_context<C>(&self) -> Option<C>
    where
        C: IoContext,
    {
        self.context.downcast_ref::<C>().cloned()
    }

    fn execute_cycle(&mut self, max_wait: Duration, sink: &Sink) -> Result<(), DriverError> {
        self.driver.execute_cycle(&mut Cycle::new(max_wait)).inspect_err(|error| {
            emit!(
                sink,
                IoDriverCycleFailed {
                    context: IoContextName(self.context_name),
                    role: IoDriverRoleLabel::from(self.role),
                    error: IoDriverErrorMessage::new(error),
                }
            );
        })
    }
}

struct InitializationGuard<'a>(&'a Cell<bool>);

impl Drop for InitializationGuard<'_> {
    fn drop(&mut self) {
        debug_assert!(self.0.replace(false), "worker-local I/O initialization guard must be active");
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
    pub(crate) fn new(runtime: IoRuntime, thread: Thread, owner_waker: Waker) -> Self {
        Self {
            runtime,
            thread,
            owner_waker,
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
        self.runtime.check_failed()?;
        if let Some(context) = self
            .primary
            .borrow()
            .as_ref()
            .and_then(LocalContext::try_clone_context::<C>)
            .or_else(|| self.secondaries.borrow().iter().find_map(LocalContext::try_clone_context::<C>))
        {
            self.runtime.check_failed()?;
            return Ok(context);
        }
        if self.initializing.replace(true) {
            return Err(DriverError::from_message(
                "I/O context retrieval cannot be nested during initialization",
            ));
        }
        let _initialization = InitializationGuard(&self.initializing);
        let Resolution { mut context, role } = self.runtime.resolve_context::<C>(options)?;
        context.relocate(None, &self.thread);
        let local = match self.create_local_driver::<C>(context, role) {
            Ok(local) => local,
            Err(error) => {
                self.runtime.fail(&error);
                return Err(error);
            }
        };
        let context = local.clone_context::<C>();
        match local.role {
            DriverRole::Primary => {
                assert!(
                    self.primary.borrow_mut().replace(local).is_none(),
                    "a worker has at most one primary I/O driver"
                );
            }
            DriverRole::Secondary => self.secondaries.borrow_mut().push(local),
        }
        self.runtime.check_failed()?;
        Ok(context)
    }

    fn create_local_driver<C>(&self, mut context: C, role: DriverRole) -> Result<LocalContext, DriverError>
    where
        C: IoContext,
    {
        let options = DriverOptions::new(self.thread.clone(), self.owner_waker.clone(), role);
        let driver = context.driver(options).inspect_err(|error| {
            emit!(
                self.runtime.sink(),
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
            self.runtime.sink(),
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
            local.execute_cycle(Duration::ZERO, self.runtime.sink())?;
        }
        if let Some(primary) = self.primary.borrow_mut().as_mut() {
            primary.execute_cycle(max_wait, self.runtime.sink())?;
        }
        Ok(())
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
                    self.runtime.sink(),
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::task::Waker;

    use arty_io_core::{Cycle, Driver, DriverError, DriverOptions, IoContext};
    use observed::Sink;
    use thread_aware::{Thread, ThreadAware, ThreadBuilder};

    use crate::IoRuntime;

    #[derive(Clone)]
    struct TestContext;

    impl ThreadAware for TestContext {
        fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
    }

    impl IoContext for TestContext {
        type ContextOptions = ();
        type Driver = TestDriver;

        fn context((): Self::ContextOptions) -> Self {
            Self
        }

        fn driver(&mut self, _options: DriverOptions) -> Result<Self::Driver, DriverError> {
            Ok(TestDriver)
        }
    }

    struct TestDriver;

    impl Driver for TestDriver {
        fn waker(&self) -> Waker {
            Waker::noop().clone()
        }

        fn execute_cycle(&mut self, _cycle: &mut Cycle) -> Result<(), DriverError> {
            Ok(())
        }

        fn shutdown(self) -> Result<(), DriverError> {
            Ok(())
        }
    }

    #[test]
    fn nested_context_retrieval_is_rejected_during_initialization() {
        let thread = ThreadBuilder::default().build(std::thread::current().id());
        let mut runtime = IoRuntime::new(Sink::noop());
        runtime.relocate(None, &thread);
        let host = runtime.driver_host(Waker::noop().clone());
        host.initializing.set(true);

        let Err(error) = host.context::<TestContext>(&()) else {
            panic!("nested context retrieval should fail");
        };

        assert_eq!(error.to_string(), "I/O context retrieval cannot be nested during initialization");
    }
}
