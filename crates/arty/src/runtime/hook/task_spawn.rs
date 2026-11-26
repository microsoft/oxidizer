// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::marker::PhantomData;
use std::rc::Rc;
use std::task::Poll;

use seismograph_runtime::TaskIdentity;
use smallvec::SmallVec;
use thread_aware::{Thread, ThreadAware};

use super::hook_fn::ErasedHookFn;

/// Observes each top-level poll of an async task.
///
/// The handler runs on one async worker. [`TaskSpawn::observe_with`] relocates
/// its input data to that worker, then creates the observer there.
///
/// The scope includes the first poll, wake-driven polls, completion, and task
/// panics. It excludes blocking functions and nested future polls inside one
/// top-level task poll. Implementations must not panic. Arty does not catch
/// panics from these methods.
pub trait TaskObserver: 'static {
    /// Runs immediately before Arty polls the task.
    fn poll_start(&mut self);

    /// Runs immediately after Arty polls the task.
    ///
    /// `outcome` is `Poll::Pending` if the task remains pending. It is
    /// `Poll::Ready(())` if the task completes or panics.
    fn poll_end(&mut self, outcome: Poll<()>);
}

/// Reports that Arty is submitting an async task.
///
/// Arty creates this event before it sends the task to its selected worker.
/// The event reports the optional origin worker and the required target worker.
/// Its debug output includes the task identity and the source location that
/// submitted the task.
/// On a runtime worker, Arty uses that worker's hook instance. Outside runtime
/// workers, Arty uses a worker-neutral hook instance. The event covers tasks submitted through
/// [`RuntimeScheduler`](crate::task::RuntimeScheduler) and
/// [`Scheduler`](crate::task::Scheduler), including worker-specific and
/// relocating submissions. Arty does not create this event for a task that it
/// rejects before hook execution. A hook can start shutdown, so registration
/// can still reject the task after the event. The callback must not panic.
#[derive(derive_more::Debug)]
pub struct TaskSpawn {
    pub(super) origin: Option<Thread>,
    pub(super) target: Thread,
    #[expect(dead_code, reason = "the immutable task identity is currently exposed only through Debug output")]
    pub(super) identity: TaskIdentity,
    #[debug(ignore)]
    pub(super) observers: Option<TaskObservers>,
}

impl TaskSpawn {
    pub(crate) fn new(origin: Option<Thread>, target: Thread, identity: TaskIdentity) -> Self {
        Self {
            origin,
            target,
            identity,
            observers: None,
        }
    }

    pub(crate) fn into_observers(self) -> Option<TaskObservers> {
        self.observers
    }

    /// Returns the worker that submitted the task.
    ///
    /// Returns `None` when the submission originated outside a runtime worker.
    #[must_use]
    pub fn origin(&self) -> Option<&Thread> {
        self.origin.as_ref()
    }

    /// Returns the worker selected to run the task.
    #[must_use]
    pub const fn target(&self) -> &Thread {
        &self.target
    }

    /// Adds a relocatable poll observer to the task.
    ///
    /// Arty relocates `observer` to the target worker before task registration.
    /// Use [`observe_with`](Self::observe_with) when the observer itself cannot
    /// move between threads.
    pub fn observe<O>(&mut self, observer: O)
    where
        O: TaskObserver + ThreadAware,
    {
        self.observe_with(observer, std::convert::identity);
    }

    /// Adds a poll observer that Arty creates on the target worker.
    ///
    /// Arty relocates `data` from [`origin`](Self::origin) to
    /// [`target`](Self::target), then calls `create` on the target worker. The
    /// created observer does not move between threads and does not need to be
    /// `Send`.
    ///
    /// The first observer is the outermost observer. The first two registrations
    /// use inline collection storage.
    pub fn observe_with<D, O>(&mut self, data: D, create: fn(D) -> O)
    where
        D: ThreadAware + 'static,
        O: TaskObserver,
    {
        let observer = Box::new(ObserverSlot::Pending { data, create }) as Box<dyn TaskObserverSlot>;
        self.observers = Some(match self.observers.take() {
            Some(observers) => observers.push(observer),
            None => TaskObservers::new(self.origin.clone(), observer),
        });
    }
}

/// Type-erased storage that crosses threads only before observer activation.
///
/// Arty relocates the pending data, activates the observer on the target
/// worker, and calls the poll methods only after activation.
trait TaskObserverSlot: ThreadAware {
    /// Creates the target-local observer from the relocated data.
    fn activate(&mut self);

    /// Forwards the start callback to the active observer.
    fn poll_start(&mut self);

    /// Forwards the end callback to the active observer.
    fn poll_end(&mut self, outcome: Poll<()>);
}

enum ObserverSlot<D, O> {
    Pending { data: D, create: fn(D) -> O },
    Activating,
    Active(O),
}

#[expect(
    clippy::non_send_fields_in_send_ty,
    reason = "O is created only after the private slot reaches its target worker and is then held behind a non-Send wrapper"
)]
// SAFETY: An ObserverSlot crosses threads only in the Pending state, which
// contains D and a function pointer. D is Send through ThreadAware. activate()
// creates O on the target worker, after which the private slot is held only by
// non-Send PollObservers or by the target executor's task instrumentation.
unsafe impl<D, O> Send for ObserverSlot<D, O> where D: ThreadAware {}

impl<D, O> ThreadAware for ObserverSlot<D, O>
where
    D: ThreadAware,
{
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        let Self::Pending { data, .. } = self else {
            unreachable!("observer data relocates before target activation");
        };
        data.relocate(source, destination);
    }
}

impl<D, O> TaskObserverSlot for ObserverSlot<D, O>
where
    D: ThreadAware + 'static,
    O: TaskObserver,
{
    fn activate(&mut self) {
        let Self::Pending { data, create } = std::mem::replace(self, Self::Activating) else {
            unreachable!("an observer slot activates exactly once");
        };
        *self = Self::Active(create(data));
    }

    fn poll_start(&mut self) {
        let Self::Active(observer) = self else {
            unreachable!("task polling starts after observer activation");
        };
        observer.poll_start();
    }

    fn poll_end(&mut self, outcome: Poll<()>) {
        let Self::Active(observer) = self else {
            unreachable!("task polling ends after observer activation");
        };
        observer.poll_end(outcome);
    }
}

#[derive(derive_more::Debug)]
pub(crate) struct TaskObservers {
    source: Option<Thread>,
    #[debug(ignore)]
    observers: SmallVec<[Box<dyn TaskObserverSlot>; 2]>,
}

impl TaskObservers {
    fn new(source: Option<Thread>, observer: Box<dyn TaskObserverSlot>) -> Self {
        let mut observers = SmallVec::new();
        observers.push(observer);
        Self { source, observers }
    }

    fn push(mut self, observer: Box<dyn TaskObserverSlot>) -> Self {
        self.observers.push(observer);
        self
    }

    pub(crate) fn relocate_to(mut self, destination: &Thread) -> PollObservers {
        for observer in &mut self.observers {
            observer.relocate(self.source.as_ref(), destination);
            observer.activate();
        }
        PollObservers {
            observers: self.observers,
            _not_send: PhantomData,
        }
    }
}

#[derive(derive_more::Debug)]
pub(crate) struct PollObservers {
    #[debug(ignore)]
    observers: SmallVec<[Box<dyn TaskObserverSlot>; 2]>,
    _not_send: PhantomData<Rc<()>>,
}

#[derive(derive_more::Debug)]
pub(crate) struct SingleObserver {
    #[debug(ignore)]
    observer: Box<dyn TaskObserverSlot>,
    _not_send: PhantomData<Rc<()>>,
}

impl PollObservers {
    pub(crate) fn into_single(mut self) -> Result<SingleObserver, Self> {
        if self.observers.len() == 1 {
            return Ok(SingleObserver {
                observer: self.observers.pop().expect("observer count was exactly one"),
                _not_send: PhantomData,
            });
        }
        Err(self)
    }

    pub(crate) fn poll_start(&mut self) {
        for observer in &mut self.observers {
            observer.poll_start();
        }
    }

    pub(crate) fn poll_end(&mut self, outcome: Poll<()>) {
        for observer in self.observers.iter_mut().rev() {
            observer.poll_end(outcome);
        }
    }
}

impl arty_executor::TaskInstrumentation for SingleObserver {
    fn poll_start(&mut self) {
        self.observer.poll_start();
    }

    fn poll_end(&mut self, outcome: Poll<()>) {
        self.observer.poll_end(outcome);
    }
}

impl arty_executor::TaskInstrumentation for PollObservers {
    fn poll_start(&mut self) {
        Self::poll_start(self);
    }

    fn poll_end(&mut self, outcome: Poll<()>) {
        Self::poll_end(self, outcome);
    }
}

impl super::Hook for TaskSpawn {}

impl From<ErasedHookFn<TaskSpawn>> for super::HookRegistration {
    fn from(hook: ErasedHookFn<TaskSpawn>) -> Self {
        Self::TaskSpawn(hook)
    }
}

impl super::private::Sealed for TaskSpawn {}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]

    #[cfg(not(miri))]
    use std::alloc::System;
    use std::hint::black_box;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[cfg(not(miri))]
    use alloc_tracker::{Allocator, Session};
    use static_assertions::{assert_impl_all, assert_not_impl_any};
    use thread_aware::ThreadBuilder;

    use super::*;

    assert_impl_all!(TaskObservers: Send);
    assert_not_impl_any!(PollObservers: Send);
    assert_not_impl_any!(SingleObserver: Send);

    #[cfg(not(miri))]
    #[global_allocator]
    static ALLOCATOR: Allocator<System> = Allocator::system();

    #[cfg(not(miri))]
    const MEASURED_OPERATIONS: usize = 1_000;

    struct OneBytePoll(u8);

    impl ThreadAware for OneBytePoll {
        fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
    }

    impl TaskObserver for OneBytePoll {
        fn poll_start(&mut self) {
            black_box(self.0);
        }

        fn poll_end(&mut self, _outcome: Poll<()>) {}
    }

    struct LocalPollData(Arc<AtomicUsize>);

    impl ThreadAware for LocalPollData {
        fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
    }

    struct LocalPoll {
        calls: Arc<AtomicUsize>,
        created_on: std::thread::ThreadId,
        _not_send: Rc<()>,
    }

    assert_not_impl_any!(LocalPoll: Send);

    impl LocalPoll {
        fn create(data: LocalPollData) -> Self {
            Self {
                calls: data.0,
                created_on: std::thread::current().id(),
                _not_send: Rc::new(()),
            }
        }
    }

    impl TaskObserver for LocalPoll {
        fn poll_start(&mut self) {
            assert_eq!(self.created_on, std::thread::current().id());
            self.calls.fetch_add(1, Ordering::Relaxed);
        }

        fn poll_end(&mut self, _outcome: Poll<()>) {}
    }

    #[derive(Debug, Eq, PartialEq)]
    enum RelocationSource {
        NotRelocated,
        External,
        Worker(std::thread::ThreadId),
    }

    struct RelocationProbe(Arc<Mutex<RelocationSource>>);

    impl ThreadAware for RelocationProbe {
        fn relocate(&mut self, source: Option<&Thread>, _destination: &Thread) {
            *self.0.lock().unwrap() = source.map_or(RelocationSource::External, |thread| RelocationSource::Worker(thread.id()));
        }
    }

    impl TaskObserver for RelocationProbe {
        fn poll_start(&mut self) {}

        fn poll_end(&mut self, _outcome: Poll<()>) {}
    }

    #[test]
    #[cfg(not(miri))]
    fn one_observer_uses_one_allocation() {
        assert_eq!(observer_allocations(1), MEASURED_OPERATIONS as u64);
    }

    #[test]
    #[cfg(not(miri))]
    fn two_observers_use_two_allocations() {
        assert_eq!(observer_allocations(2), (MEASURED_OPERATIONS * 2) as u64);
    }

    #[test]
    #[cfg(not(miri))]
    fn third_observer_allocates_collection_buffer() {
        assert_eq!(observer_allocations(3), (MEASURED_OPERATIONS * 4) as u64);
    }

    #[test]
    fn into_single_returns_only_observer() {
        let worker = ThreadBuilder::default().build(std::thread::current().id());
        let mut event = TaskSpawn::new(None, worker.clone(), TaskIdentity::from_location(std::panic::Location::caller()));
        event.observe(OneBytePoll(1));

        let _observer = event.into_observers().unwrap().relocate_to(&worker).into_single().unwrap();
    }

    #[test]
    fn into_single_returns_original_collection_for_multiple_observers() {
        let worker = ThreadBuilder::default().build(std::thread::current().id());
        let mut event = TaskSpawn::new(None, worker.clone(), TaskIdentity::from_location(std::panic::Location::caller()));
        event.observe(OneBytePoll(1));
        event.observe(OneBytePoll(2));
        let observers = event.into_observers().unwrap().relocate_to(&worker);

        let Err(observers) = observers.into_single() else {
            panic!("multiple observers cannot become one observer");
        };
        assert_eq!(observers.observers.len(), 2);
    }

    #[test]
    fn relocation_uses_the_captured_source_worker() {
        let source = ThreadBuilder::default().build(std::thread::current().id());
        let destination = ThreadBuilder::default().build(std::thread::current().id());
        let observed = Arc::new(Mutex::new(RelocationSource::NotRelocated));
        let mut event = TaskSpawn::new(
            Some(source.clone()),
            destination.clone(),
            TaskIdentity::from_location(std::panic::Location::caller()),
        );
        event.observe(RelocationProbe(Arc::clone(&observed)));

        let _observer = event.into_observers().unwrap().relocate_to(&destination).into_single().unwrap();

        assert_eq!(*observed.lock().unwrap(), RelocationSource::Worker(source.id()));
    }

    #[test]
    fn observe_with_creates_a_non_send_observer_on_the_target() {
        let target = ThreadBuilder::default().build(std::thread::current().id());
        let calls = Arc::new(AtomicUsize::new(0));
        let mut event = TaskSpawn::new(None, target.clone(), TaskIdentity::from_location(std::panic::Location::caller()));
        event.observe_with(LocalPollData(Arc::clone(&calls)), LocalPoll::create);

        let mut observer = event.into_observers().unwrap().relocate_to(&target).into_single().unwrap();
        arty_executor::TaskInstrumentation::poll_start(&mut observer);

        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn observer_slots_reject_invalid_lifecycle_order() {
        let target = ThreadBuilder::default().build(std::thread::current().id());

        assert_panics(|| {
            let mut slot = ObserverSlot::<(), OneBytePoll>::Activating;
            slot.relocate(None, &target);
        });
        assert_panics(|| {
            let mut slot = ObserverSlot::<(), OneBytePoll>::Activating;
            slot.activate();
        });
        assert_panics(|| {
            let mut slot = ObserverSlot::Pending {
                data: (),
                create: |()| OneBytePoll(1),
            };
            slot.poll_start();
        });
        assert_panics(|| {
            let mut slot = ObserverSlot::Pending {
                data: (),
                create: |()| OneBytePoll(1),
            };
            slot.poll_end(Poll::Pending);
        });
    }

    fn assert_panics(action: impl FnOnce()) {
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(action)).is_err());
    }

    #[cfg(not(miri))]
    fn observer_allocations(observer_count: usize) -> u64 {
        let worker = ThreadBuilder::default().build(std::thread::current().id());
        let session = Session::new().no_stdout().no_file();
        let operation = session.operation("observers");
        {
            let _span = operation.measure_thread().iterations(MEASURED_OPERATIONS as u64);
            for value in 0..MEASURED_OPERATIONS {
                let mut event = TaskSpawn::new(
                    Some(worker.clone()),
                    worker.clone(),
                    TaskIdentity::from_location(std::panic::Location::caller()),
                );
                for observer in 0..observer_count {
                    event.observe(OneBytePoll(u8::try_from((value + observer) % 256).unwrap()));
                }
                black_box(event.into_observers().unwrap().relocate_to(&worker));
            }
        }

        session
            .to_report()
            .operations()
            .find_map(|(name, operation)| (name == "observers").then(|| operation.total_allocations_count()))
            .unwrap()
    }
}
