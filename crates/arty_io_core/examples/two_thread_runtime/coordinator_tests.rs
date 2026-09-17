// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::TypeId;
use std::cell::{Cell, RefCell};
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use std::task::Waker;
use std::thread;
use std::time::{Duration, Instant};

use arty_io_core::{CompletionBudget, CompletionWaiter, Drain, DrainStatus, Driver, DriverError, LocalDriver, ServiceStatus, WaitStatus};

use super::{Coordinator, ErasedDriver, Source};
use crate::test_support::ManualTasks;

struct A;
struct B;

struct ObservedWaiter {
    waits: Rc<RefCell<Vec<Duration>>>,
    status: ServiceStatus,
}

impl CompletionWaiter for ObservedWaiter {
    fn waker(&self) -> Waker {
        Waker::noop().clone()
    }

    fn collect(&mut self, max_wait: Duration, budget: &mut CompletionBudget) -> Result<ServiceStatus, DriverError> {
        assert!(budget.try_consume());
        self.waits.borrow_mut().push(max_wait);
        Ok(self.status)
    }
}

type Service = Box<dyn FnMut(&mut CompletionBudget) -> Result<ServiceStatus, DriverError>>;
type DrainService = Box<dyn FnMut(&mut CompletionBudget) -> Result<DrainStatus, DriverError>>;
type Arm = Box<dyn FnMut() -> Result<WaitStatus, DriverError>>;

struct Scripted<S: Drain = ScriptedDrain> {
    service: Service,
    arm: Arm,
    shutdown: Box<dyn FnOnce() -> S>,
}

impl Scripted<ScriptedDrain> {
    fn new(service: Service) -> Self {
        Self {
            service,
            arm: Box::new(|| Ok(WaitStatus::Armed)),
            shutdown: Box::new(|| ScriptedDrain {
                service: Box::new(|budget| {
                    assert!(budget.try_consume());
                    Ok(DrainStatus::Complete)
                }),
                arm: Box::new(|| Ok(WaitStatus::WorkReady)),
            }),
        }
    }
}

impl<S: Drain> Driver for Scripted<S> {
    type Drain = S;

    fn service(&mut self, budget: &mut CompletionBudget) -> Result<ServiceStatus, DriverError> {
        (self.service)(budget)
    }

    fn prepare_wait(&mut self) -> Result<WaitStatus, DriverError> {
        (self.arm)()
    }

    fn shutdown(self) -> Self::Drain {
        (self.shutdown)()
    }
}

fn erase(driver: impl Driver) -> Box<dyn ErasedDriver> {
    Box::new(LocalDriver::new(driver))
}

struct ScriptedDrain {
    service: DrainService,
    arm: Arm,
}

#[derive(Clone, Copy)]
enum Terminal {
    Complete,
    ServiceError,
    PreparationError,
}

struct CountedDrain {
    terminal: Terminal,
    services: Rc<Cell<usize>>,
    preparations: Rc<Cell<usize>>,
    drops: Rc<Cell<usize>>,
}

impl Drain for CountedDrain {
    fn service(&mut self, budget: &mut CompletionBudget) -> Result<DrainStatus, DriverError> {
        assert!(budget.try_consume());
        self.services.set(self.services.get() + 1);
        match self.terminal {
            Terminal::Complete => Ok(DrainStatus::Complete),
            Terminal::ServiceError => Err(DriverError::from_message("terminal drain service failure")),
            Terminal::PreparationError => Ok(DrainStatus::Pending(ServiceStatus::Idle)),
        }
    }

    fn prepare_wait(&mut self) -> Result<WaitStatus, DriverError> {
        self.preparations.set(self.preparations.get() + 1);
        Err(DriverError::from_message("terminal drain preparation failure"))
    }
}

impl Drop for CountedDrain {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}

impl Drain for ScriptedDrain {
    fn service(&mut self, budget: &mut CompletionBudget) -> Result<DrainStatus, DriverError> {
        (self.service)(budget)
    }

    fn prepare_wait(&mut self) -> Result<WaitStatus, DriverError> {
        (self.arm)()
    }
}

fn coordinator(quantum: usize) -> Coordinator<ObservedWaiter> {
    Coordinator::new(
        ObservedWaiter {
            waits: Rc::default(),
            status: ServiceStatus::Idle,
        },
        NonZeroUsize::new(quantum).unwrap(),
    )
}

#[test]
fn installation_schedules_an_initial_turn_without_a_notification() {
    let mut coordinator = coordinator(1);
    let source = Source::new(coordinator.waiter.waker());
    let calls = Rc::new(Cell::new(0));
    let service_calls = Rc::clone(&calls);
    let arm_calls = Rc::clone(&calls);
    let mut driver = Scripted::new(Box::new(move |budget| {
        assert!(budget.try_consume());
        service_calls.set(service_calls.get() + 1);
        Ok(ServiceStatus::Idle)
    }));
    driver.arm = Box::new(move || {
        assert_eq!(arm_calls.get(), 1);
        Ok(WaitStatus::Armed)
    });
    coordinator.insert(TypeId::of::<A>(), Arc::clone(&source), erase(driver));
    assert!(!source.is_ready());
    assert_eq!(coordinator.wait_duration(Instant::now(), None), Duration::ZERO);
    coordinator.service(Instant::now());
    assert_eq!(calls.get(), 1);
    assert!(!source.is_ready());
    coordinator.arm();
    assert_eq!(coordinator.wait_duration(Instant::now(), None), Duration::MAX);
    coordinator.service(Instant::now());
    assert_eq!(calls.get(), 1);
}

#[test]
fn shutdown_of_an_idle_driver_schedules_its_own_initial_turn() {
    let mut coordinator = coordinator(1);
    let source = Source::new(coordinator.waiter.waker());
    let calls = Rc::new(Cell::new(0));
    let service_calls = Rc::clone(&calls);
    let arm_calls = Rc::clone(&calls);
    let mut driver = Scripted::new(Box::new(|_| Ok(ServiceStatus::Idle)));
    driver.shutdown = Box::new(move || ScriptedDrain {
        service: Box::new(move |budget| {
            assert!(budget.try_consume());
            service_calls.set(service_calls.get() + 1);
            Ok(DrainStatus::Pending(ServiceStatus::Idle))
        }),
        arm: Box::new(move || {
            assert_eq!(arm_calls.get(), 1);
            Ok(WaitStatus::Armed)
        }),
    });
    coordinator.insert(TypeId::of::<A>(), Arc::clone(&source), erase(driver));
    coordinator.service(Instant::now());
    coordinator.arm();
    assert_eq!(coordinator.wait_duration(Instant::now(), None), Duration::MAX);
    coordinator.begin_shutdown_all();
    assert!(!source.is_ready());
    assert_eq!(coordinator.wait_duration(Instant::now(), None), Duration::ZERO);
    coordinator.service(Instant::now());
    assert_eq!(calls.get(), 1);
    coordinator.arm();
    assert_eq!(coordinator.wait_duration(Instant::now(), None), Duration::MAX);
    coordinator.service(Instant::now());
    assert_eq!(calls.get(), 1);
}

#[test]
fn shutdown_created_by_driver_failure_also_gets_an_initial_turn() {
    for fail_during_arm in [false, true] {
        let mut coordinator = coordinator(1);
        let source = Source::new(coordinator.waiter.waker());
        let mut driver = Scripted::new(Box::new(move |budget| {
            assert!(budget.try_consume());
            if fail_during_arm {
                Ok(ServiceStatus::Idle)
            } else {
                Err(DriverError::from_message("injected driver service failure"))
            }
        }));
        driver.arm = Box::new(|| Err(DriverError::from_message("injected driver arming failure")));
        coordinator.insert(TypeId::of::<A>(), Arc::clone(&source), erase(driver));
        coordinator.service(Instant::now());
        if fail_during_arm {
            coordinator.arm();
        }
        assert!(coordinator.has_failed());
        assert!(!source.is_ready());
        assert_eq!(coordinator.wait_duration(Instant::now(), None), Duration::ZERO);
        coordinator.service(Instant::now());
        assert!(coordinator.is_empty());
        let retired = coordinator.take_retired();
        assert_eq!(retired.len(), 1);
        assert_eq!(retired[0].errors.len(), 1);
    }
}

#[test]
fn a_wake_during_service_is_not_cleared_by_its_idle_result() {
    let mut coordinator = coordinator(1);
    let source = Source::new(coordinator.waiter.waker());
    let wake = Waker::from(Arc::clone(&source));
    let calls = Rc::new(Cell::new(0));
    let service_calls = Rc::clone(&calls);
    coordinator.insert(
        TypeId::of::<A>(),
        Arc::clone(&source),
        erase(Scripted::new(Box::new(move |budget| {
            assert!(budget.try_consume());
            service_calls.set(service_calls.get() + 1);
            if service_calls.get() == 1 {
                wake.wake_by_ref();
            }
            Ok(ServiceStatus::Idle)
        }))),
    );
    coordinator.service(Instant::now());
    assert!(source.is_ready());
    assert_eq!(coordinator.wait_duration(Instant::now(), None), Duration::ZERO);
    coordinator.service(Instant::now());
    assert!(!source.is_ready());
    assert_eq!(calls.get(), 2);
    assert_eq!(coordinator.wait_duration(Instant::now(), None), Duration::MAX);
}

#[test]
fn a_notification_during_arm_is_rechecked_before_waiting() {
    let mut coordinator = coordinator(1);
    let source = Source::new(coordinator.waiter.waker());
    let wake = Waker::from(Arc::clone(&source));
    let (armed_tx, armed_rx) = mpsc::channel();
    let (notified_tx, notified_rx) = mpsc::channel();
    let notifier = thread::spawn(move || {
        armed_rx.recv().unwrap();
        wake.wake();
        notified_tx.send(()).unwrap();
    });
    let mut driver = Scripted::new(Box::new(|_| Ok(ServiceStatus::Idle)));
    driver.arm = Box::new(move || {
        armed_tx.send(()).unwrap();
        notified_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        Ok(WaitStatus::Armed)
    });
    coordinator.insert(TypeId::of::<A>(), source, erase(driver));
    coordinator.service(Instant::now());
    coordinator.arm();
    assert_eq!(coordinator.wait_duration(Instant::now(), None), Duration::ZERO);
    notifier.join().unwrap();
}

#[test]
fn collection_and_every_busy_source_get_a_bounded_turn() {
    let mut coordinator = coordinator(2);
    let work = Rc::new(RefCell::new(Vec::new()));
    for (id, name) in [(TypeId::of::<A>(), 'a'), (TypeId::of::<B>(), 'b')] {
        let work = Rc::clone(&work);
        coordinator.insert(
            id,
            Source::new(coordinator.waiter.waker()),
            erase(Scripted::new(Box::new(move |budget| {
                for _ in 0..2 {
                    assert!(budget.try_consume());
                    work.borrow_mut().push(name);
                }
                assert!(!budget.try_consume());
                Ok(ServiceStatus::Runnable)
            }))),
        );
    }
    for round in 1..=5 {
        coordinator.service(Instant::now());
        assert_eq!(work.borrow().iter().filter(|&&name| name == 'a').count(), round * 2);
        assert_eq!(work.borrow().iter().filter(|&&name| name == 'b').count(), round * 2);
        assert_eq!(coordinator.waiter.waits.borrow().len(), round);
        assert_eq!(coordinator.wait_duration(Instant::now(), None), Duration::ZERO);
    }
    assert!(coordinator.waiter.waits.borrow().iter().all(Duration::is_zero));
    assert_eq!(&work.borrow()[..8], &['a', 'a', 'b', 'b', 'b', 'b', 'a', 'a']);
}

#[test]
fn service_deadlines_survive_idle_rounds_and_bound_the_domain_wait() {
    let mut coordinator = coordinator(1);
    let now = Instant::now();
    let deadline = now + Duration::from_secs(10);
    let calls = Rc::new(Cell::new(0));
    let service_calls = Rc::clone(&calls);
    coordinator.insert(
        TypeId::of::<A>(),
        Source::new(coordinator.waiter.waker()),
        erase(Scripted::new(Box::new(move |budget| {
            assert!(budget.try_consume());
            service_calls.set(service_calls.get() + 1);
            Ok(if service_calls.get() == 1 {
                ServiceStatus::Deadline(deadline)
            } else {
                ServiceStatus::Idle
            })
        }))),
    );
    coordinator.service(now);
    coordinator.arm();
    assert_eq!(coordinator.wait_duration(now, None), Duration::from_secs(10));
    assert_eq!(
        coordinator.wait_duration(now, Some(now + Duration::from_secs(2))),
        Duration::from_secs(2)
    );
    coordinator.service(now + Duration::from_secs(3));
    assert_eq!(calls.get(), 1);
    assert_eq!(coordinator.wait_duration(deadline, None), Duration::ZERO);
    coordinator.service(deadline);
    assert_eq!(calls.get(), 2);
    assert_eq!(coordinator.wait_duration(deadline, None), Duration::MAX);
}

#[test]
fn an_expired_collector_deadline_prevents_sleeping() {
    let mut coordinator = coordinator(1);
    let now = Instant::now();
    coordinator.waiter.status = ServiceStatus::Deadline(now + Duration::from_secs(4));
    coordinator.service(now);
    assert_eq!(coordinator.wait_duration(now, None), Duration::from_secs(4));
    assert_eq!(coordinator.wait_duration(now + Duration::from_secs(4), None), Duration::ZERO);
}

#[test]
fn a_retained_old_source_never_notifies_its_replacement() {
    let mut coordinator = coordinator(1);
    let old = Source::new(coordinator.waiter.waker());
    coordinator.insert(
        TypeId::of::<A>(),
        Arc::clone(&old),
        erase(Scripted::new(Box::new(|_| Ok(ServiceStatus::Idle)))),
    );
    assert!(coordinator.begin_shutdown(TypeId::of::<A>()));
    coordinator.service(Instant::now());
    assert!(coordinator.is_empty());
    let replacement = Source::new(coordinator.waiter.waker());
    let calls = Rc::new(Cell::new(0));
    let service_calls = Rc::clone(&calls);
    coordinator.insert(
        TypeId::of::<A>(),
        Arc::clone(&replacement),
        erase(Scripted::new(Box::new(move |_| {
            service_calls.set(service_calls.get() + 1);
            Ok(ServiceStatus::Idle)
        }))),
    );
    coordinator.service(Instant::now());
    Waker::from(old).wake();
    coordinator.service(Instant::now());
    assert_eq!(calls.get(), 1);
    assert!(!replacement.is_ready());
}

#[test]
fn drain_failure_does_not_serialize_or_abandon_the_other_drain() {
    let mut coordinator = coordinator(1);
    let starts = Rc::new(RefCell::new(Vec::new()));
    let tasks = ManualTasks::default();
    let resource = Arc::new(String::from("retained callback data"));
    let weak = Arc::downgrade(&resource);
    let (callback_tx, callback_rx) = mpsc::channel();
    let task_resource = Arc::clone(&resource);
    tasks
        .handle()
        .spawn(move || callback_tx.send(task_resource.to_string()).unwrap())
        .unwrap();
    for (id, name) in [(TypeId::of::<A>(), 'a'), (TypeId::of::<B>(), 'b')] {
        let starts = Rc::clone(&starts);
        let retained = Arc::clone(&resource);
        let mut driver = Scripted::new(Box::new(|_| Ok(ServiceStatus::Idle)));
        driver.shutdown = Box::new(move || {
            starts.borrow_mut().push(name);
            let mut turns = 0;
            ScriptedDrain {
                service: Box::new(move |budget| {
                    assert!(budget.try_consume());
                    assert!(!retained.is_empty());
                    turns += 1;
                    if name == 'a' {
                        return Err(DriverError::from_message("injected drain failure"));
                    }
                    Ok(if turns == 2 {
                        DrainStatus::Complete
                    } else {
                        DrainStatus::Pending(ServiceStatus::Runnable)
                    })
                }),
                arm: Box::new(|| Ok(WaitStatus::Armed)),
            }
        });
        coordinator.insert(id, Source::new(coordinator.waiter.waker()), erase(driver));
    }
    drop(resource);
    coordinator.begin_shutdown_all();
    assert_eq!(*starts.borrow(), vec!['a', 'b']);
    coordinator.service(Instant::now());
    assert!(coordinator.has_failed());
    assert!(!coordinator.is_empty());
    coordinator.service(Instant::now());
    assert!(coordinator.is_empty());
    let results = coordinator.take_retired();
    assert_eq!(results.len(), 2);
    assert_eq!(results.iter().filter(|result| !result.errors.is_empty()).count(), 1);
    assert!(weak.upgrade().is_some());
    tasks.run_all();
    assert_eq!(callback_rx.recv().unwrap(), "retained callback data");
    assert!(weak.upgrade().is_none());
}

#[test]
fn a_terminal_arm_failure_releases_and_retires_the_drain() {
    let mut coordinator = coordinator(1);
    let calls = Rc::new(Cell::new(0));
    let service_calls = Rc::clone(&calls);
    let arms = Rc::new(Cell::new(0));
    let arm_calls = Rc::clone(&arms);
    let resource = Rc::new(Cell::new(false));
    let weak_resource = Rc::downgrade(&resource);
    let mut driver = Scripted::new(Box::new(|_| Ok(ServiceStatus::Idle)));
    driver.shutdown = Box::new(move || ScriptedDrain {
        service: Box::new(move |budget| {
            assert!(budget.try_consume());
            resource.set(true);
            service_calls.set(service_calls.get() + 1);
            Ok(DrainStatus::Pending(ServiceStatus::Idle))
        }),
        arm: Box::new(move || {
            arm_calls.set(arm_calls.get() + 1);
            Err(DriverError::from_message("injected drain arming failure"))
        }),
    });
    coordinator.insert(TypeId::of::<A>(), Source::new(coordinator.waiter.waker()), erase(driver));
    coordinator.begin_shutdown_all();
    coordinator.service(Instant::now());
    assert!(weak_resource.upgrade().is_some());
    coordinator.arm();
    assert!(weak_resource.upgrade().is_none());
    coordinator.service(Instant::now());
    coordinator.arm();
    coordinator.service(Instant::now());
    assert!(coordinator.is_empty());
    assert_eq!(calls.get(), 1);
    assert_eq!(arms.get(), 1);
    let retired = coordinator.take_retired();
    assert_eq!(retired.len(), 1);
    assert_eq!(retired[0].errors.len(), 1);
    assert_eq!(retired[0].errors[0].to_string(), "injected drain arming failure");
}

#[test]
fn every_terminal_drain_result_drops_once_without_further_calls_or_restart() {
    for terminal in [Terminal::Complete, Terminal::ServiceError, Terminal::PreparationError] {
        let mut coordinator = coordinator(1);
        let services = Rc::new(Cell::new(0));
        let preparations = Rc::new(Cell::new(0));
        let drops = Rc::new(Cell::new(0));
        let starts = Rc::new(Cell::new(0));
        let drain = CountedDrain {
            terminal,
            services: Rc::clone(&services),
            preparations: Rc::clone(&preparations),
            drops: Rc::clone(&drops),
        };
        let shutdown_starts = Rc::clone(&starts);
        let driver = Scripted {
            service: Box::new(|_| Ok(ServiceStatus::Idle)),
            arm: Box::new(|| Ok(WaitStatus::Armed)),
            shutdown: Box::new(move || {
                shutdown_starts.set(shutdown_starts.get() + 1);
                drain
            }),
        };
        coordinator.insert(TypeId::of::<A>(), Source::new(coordinator.waiter.waker()), erase(driver));
        coordinator.begin_shutdown_all();
        coordinator.begin_shutdown_all();
        coordinator.service(Instant::now());
        coordinator.arm();
        assert_eq!(drops.get(), 1);
        assert!(coordinator.is_empty());
        for _ in 0..3 {
            coordinator.begin_shutdown_all();
            coordinator.service(Instant::now());
            coordinator.arm();
        }
        assert_eq!(starts.get(), 1);
        assert_eq!(services.get(), 1);
        assert_eq!(preparations.get(), usize::from(matches!(terminal, Terminal::PreparationError)));
        assert_eq!(drops.get(), 1);
        let retired = coordinator.take_retired();
        assert_eq!(retired.len(), 1);
        assert_eq!(retired[0].errors.len(), usize::from(!matches!(terminal, Terminal::Complete)));
    }
}
