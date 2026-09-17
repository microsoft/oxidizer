// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public contract coverage for completion coordination and owned draining.
//!
//! These tests exercise preserved scenarios rather than the existence of particular types.
//! Waits are bounded and verified through explicit counters, so mutated budget or latch
//! accounting fails a test promptly instead of blocking the harness.

#![expect(clippy::unwrap_used, reason = "test failures should report the failed operation directly")]

use std::cell::Cell;
use std::error::Error;
use std::marker::PhantomData;
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::task::{Wake, Waker};
use std::time::{Duration, Instant};
use std::{fmt, io, thread};

use arty_io_core::{
    CompletionBudget, CompletionWaiter, Drain, DrainStatus, Driver, DriverContext, DriverError, DriverProvider, IoContext, LocalDrain,
    LocalDriver, ServiceStatus, SystemTask, SystemTasks, WaitStatus,
};
use static_assertions::{assert_impl_all, assert_not_impl_any};
use thread_aware_core::{Thread, ThreadAware};

type InstalledDriver = LocalDriver<TestDriver>;
type InstalledDrain = LocalDrain<TestDrain>;

/// Every cooperative loop in this file is bounded so a mutated quota cannot spin forever.
const MAX_TURNS: usize = 32;
/// No test ever sleeps longer than this, even when it asks for an unbounded wait.
const WAIT_CAP: Duration = Duration::from_millis(50);

assert_not_impl_any!(DriverContext: Send, Sync);
assert_not_impl_any!(CompletionBudget: Clone, Copy);
assert_not_impl_any!(InstalledDriver: Send, Sync, Clone);
assert_not_impl_any!(InstalledDrain: Send, Sync, Clone);
assert_impl_all!(TestDriver: Send, Sync);
assert_impl_all!(TestDrain: Send, Sync);
assert_impl_all!(SystemTask: Send, fmt::Debug);
assert_impl_all!(DriverError: Send, Sync, fmt::Debug, fmt::Display, Error);
assert_impl_all!(SystemTasks: Clone, Send, Sync, fmt::Debug);
assert_impl_all!(ServiceStatus: Copy, Clone, Eq, fmt::Debug);
assert_impl_all!(DrainStatus: Copy, Clone, Eq, fmt::Debug);
assert_impl_all!(WaitStatus: Copy, Clone, Eq, fmt::Debug);
assert_impl_all!(DualPhase: Send, Sync);
assert_impl_all!(LocalDriver<DualPhase>: Unpin);
assert_impl_all!(LocalDrain<DualPhase>: Unpin);
assert_not_impl_any!(LocalDriver<DualPhase>: Send, Sync, Drain);
assert_not_impl_any!(LocalDrain<DualPhase>: Send, Sync, Driver);

#[test]
fn local_owners_preserve_inline_layout_without_driver_boxing() {
    assert_eq!(size_of::<LocalDriver<DualPhase>>(), size_of::<DualPhase>());
    assert_eq!(align_of::<LocalDriver<DualPhase>>(), align_of::<DualPhase>());
    assert_eq!(size_of::<LocalDrain<DualPhase>>(), size_of::<DualPhase>());
    assert_eq!(align_of::<LocalDrain<DualPhase>>(), align_of::<DualPhase>());
    assert_eq!(size_of::<InstalledDriver>(), size_of::<TestDriver>());
    assert_eq!(size_of::<InstalledDrain>(), size_of::<TestDrain>());
}

#[test]
fn homogeneous_same_type_drivers_and_drains_have_unambiguous_static_phases() {
    let mut drivers = [
        LocalDriver::new(DualPhase { closed: false }),
        LocalDriver::new(DualPhase { closed: false }),
    ];
    for driver in &mut drivers {
        assert_eq!(driver.service(&mut budget(1)).unwrap(), ServiceStatus::Idle);
        assert_eq!(driver.prepare_wait().unwrap(), WaitStatus::Armed);
    }
    for mut drain in drivers.map(LocalDriver::shutdown) {
        let mut exhausted = budget(1);
        assert!(exhausted.try_consume());
        assert_eq!(
            drain.service(&mut exhausted).unwrap(),
            DrainStatus::Pending(ServiceStatus::Runnable)
        );
        assert_eq!(drain.prepare_wait().unwrap(), WaitStatus::WorkReady);
        assert_eq!(drain.service(&mut budget(1)).unwrap(), DrainStatus::Complete);
        drop(drain);
    }
}

#[test]
fn a_generic_runtime_can_store_both_phases_without_erasure() {
    enum Participant<D: Driver> {
        Driver(LocalDriver<D>),
        Drain(LocalDrain<D::Drain>),
    }

    fn begin_drain<D: Driver>(participant: Participant<D>) -> Participant<D> {
        match participant {
            Participant::Driver(driver) => Participant::Drain(driver.shutdown()),
            Participant::Drain(drain) => Participant::Drain(drain),
        }
    }

    let participant = Participant::Driver(LocalDriver::new(DualPhase { closed: false }));
    let participant = begin_drain(participant);
    let participant = begin_drain(participant);
    let Participant::Drain(mut drain) = participant else {
        panic!("the runtime must retain the draining phase");
    };
    assert_eq!(drain.service(&mut budget(1)).unwrap(), DrainStatus::Complete);
}

#[test]
fn budget_is_finite_and_exhaustion_does_not_underflow() {
    let mut budget = budget(2);
    assert_eq!(budget.remaining(), 2);
    assert!(budget.try_consume());
    assert_eq!(budget.remaining(), 1);
    assert!(budget.try_consume());
    assert_eq!(budget.remaining(), 0);
    assert!(!budget.try_consume());
    assert!(!budget.try_consume());
    assert_eq!(budget.remaining(), 0);
    assert!(format!("{budget:?}").contains("CompletionBudget"));
}

#[test]
fn service_status_separates_continuation_from_deadlines() {
    let deadline = Instant::now();
    assert_ne!(ServiceStatus::Idle, ServiceStatus::Runnable);
    assert_ne!(ServiceStatus::Runnable, ServiceStatus::Deadline(deadline));
    assert_eq!(ServiceStatus::Deadline(deadline), ServiceStatus::Deadline(deadline));

    // A drain reports the same scheduling needs until it reaches its retirement boundary.
    assert_ne!(DrainStatus::Pending(ServiceStatus::Runnable), DrainStatus::Complete);
    assert_eq!(
        DrainStatus::Pending(ServiceStatus::Deadline(deadline)),
        DrainStatus::Pending(ServiceStatus::Deadline(deadline))
    );
    assert_ne!(WaitStatus::Armed, WaitStatus::WorkReady);
}

#[test]
fn driver_context_exposes_worker_system_work_and_readiness() {
    let (tasks_tx, tasks_rx) = mpsc::channel();
    let wake = Arc::new(WakeCount::default());
    let context = DriverContext::new(
        worker_thread(),
        SystemTasks::new(move |task| {
            tasks_tx.send(()).unwrap();
            task.run();
            Ok(())
        }),
        Waker::from(Arc::clone(&wake)),
    );

    assert_eq!(context.thread().id(), thread::current().id());

    let (ran_tx, ran_rx) = mpsc::channel();
    context.system_tasks().spawn(move || ran_tx.send(()).unwrap()).unwrap();
    tasks_rx.recv_timeout(WAIT_CAP).unwrap();
    ran_rx.recv_timeout(WAIT_CAP).unwrap();

    context.readiness_waker().wake_by_ref();
    assert_eq!(wake.count(), 1);
    assert!(format!("{context:?}").contains("DriverContext"));
    assert!(format!("{:?}", context.system_tasks()).contains("SystemTasks"));
}

#[test]
fn system_task_acceptance_does_not_imply_completion() {
    let (sender, receiver) = mpsc::channel();
    let tasks = SystemTasks::new(move |task| {
        sender.send(task).unwrap();
        Ok(())
    });
    let ran = Arc::new(AtomicBool::new(false));
    let task_ran = Arc::clone(&ran);
    tasks.spawn(move || task_ran.store(true, Ordering::Relaxed)).unwrap();
    assert!(!ran.load(Ordering::Relaxed));
    receiver.recv_timeout(WAIT_CAP).unwrap().run();
    assert!(ran.load(Ordering::Relaxed));
}

#[test]
fn an_opaque_system_task_runs_only_when_consumed() {
    let ran = Arc::new(AtomicBool::new(false));
    let task_ran = Arc::clone(&ran);
    let task = SystemTask::new(move || task_ran.store(true, Ordering::Relaxed));
    assert!(format!("{task:?}").contains("SystemTask"));
    assert!(!ran.load(Ordering::Relaxed));
    task.run();
    assert!(ran.load(Ordering::Relaxed));
}

#[test]
fn rejected_system_work_preserves_classification_and_cause_without_executing() {
    let tasks = SystemTasks::new(|task| {
        drop(task);
        Err(DriverError::unsupported("system-task admission rejected")
            .with_cause(io::Error::new(io::ErrorKind::PermissionDenied, "native executor denial")))
    });
    let ran = Arc::new(AtomicBool::new(false));
    let task_ran = Arc::clone(&ran);
    let captures = Arc::new(String::from("rejected task state"));
    let weak = Arc::downgrade(&captures);
    let error = tasks
        .spawn(move || {
            task_ran.store(true, Ordering::Relaxed);
            drop(captures);
        })
        .unwrap_err();
    assert!(error.is_unsupported());
    assert_eq!(error.to_string(), "system-task admission rejected");
    let cause = error.source().unwrap().downcast_ref::<io::Error>().unwrap();
    assert_eq!(cause.kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(cause.to_string(), "native executor denial");
    assert!(!ran.load(Ordering::Relaxed));
    assert!(weak.upgrade().is_none());
}

#[test]
fn duplicate_client_type_is_rejected_instead_of_replaced() {
    let first = PreferredClient::new();
    let second = PreferredClient::new();
    let context = driver_context().with_completion_service(first.clone()).unwrap();

    let error = context.with_completion_service(second.clone()).unwrap_err();
    assert!(error.is_duplicate_completion_service());
    assert!(!error.is_unsupported());
    assert!(error.to_string().contains("more than once"));

    // The rejected insertion also proves the original client was not swapped underneath a driver.
    assert_eq!(first.registrations(), 0);
    assert_eq!(second.registrations(), 0);
}

#[test]
fn missing_client_is_classified_unsupported() {
    let error = driver_context().take_completion_service::<PreferredClient>().unwrap_err();
    assert!(error.is_unsupported());
    assert!(!error.is_duplicate_completion_service());
    assert!(error.source().is_none());
}

#[test]
fn a_non_clone_client_transfers_ownership_and_drops_exactly_once() {
    assert_not_impl_any!(RecordLease: Send, Sync, Clone);
    let retired = Arc::new(AtomicUsize::new(0));
    let mut context = driver_context()
        .with_completion_service(RecordLease::new(Arc::clone(&retired)))
        .unwrap();

    let taken = context.take_completion_service::<RecordLease>().unwrap();
    assert_eq!(retired.load(Ordering::Relaxed), 0, "extraction alone has no side effect");
    let error = context.take_completion_service::<RecordLease>().unwrap_err();
    assert!(error.is_unsupported());
    drop(context);
    assert_eq!(retired.load(Ordering::Relaxed), 0, "the extracted lease outlives its context");
    drop(taken);
    assert_eq!(retired.load(Ordering::Relaxed), 1, "the moved-out lease drops exactly once");
}

#[test]
fn take_once_reports_missing_on_repeat_while_a_sibling_client_stays_available() {
    let mut context = driver_context()
        .with_completion_service(PreferredClient::new())
        .unwrap()
        .with_completion_service(FallbackClient::new())
        .unwrap();

    let first_take = context.take_completion_service::<PreferredClient>().unwrap();
    let repeat = context.take_completion_service::<PreferredClient>().unwrap_err();
    assert!(repeat.is_unsupported());
    assert!(!repeat.is_duplicate_completion_service());
    drop(first_take);

    // The unrelated type was never touched by the repeated take of the first type.
    let sibling = context.take_completion_service::<FallbackClient>().unwrap();
    assert_eq!(sibling.registrations(), 0);
}

#[test]
fn reinsertion_succeeds_once_the_prior_occupant_is_taken() {
    let first = PreferredClient::new();
    let second = PreferredClient::new();
    let mut context = driver_context().with_completion_service(first.clone()).unwrap();

    // Duplicate rejection reflects current occupancy (see
    // `duplicate_client_type_is_rejected_instead_of_replaced`); taking the occupant clears it.
    let taken = context.take_completion_service::<PreferredClient>().unwrap();
    assert_eq!(taken.registrations(), 0);
    drop(taken);

    let mut context = context.with_completion_service(second.clone()).unwrap();
    context
        .take_completion_service::<PreferredClient>()
        .unwrap()
        .register(&Waker::noop().clone())
        .unwrap();
    assert_eq!(second.registrations(), 1);
    assert_eq!(first.registrations(), 0);
}

#[test]
fn missing_second_required_client_performs_no_native_effects_and_releases_the_first() {
    let retired = Arc::new(AtomicUsize::new(0));
    let registrations = Cell::new(0);
    let mut context = driver_context()
        .with_completion_service(RecordLease::new(Arc::clone(&retired)))
        .unwrap();

    let create = (|| {
        let first = context.take_completion_service::<RecordLease>()?;
        let second = context.take_completion_service::<FallbackClient>()?;
        registrations.set(registrations.get() + 1);
        second.register(context.readiness_waker())?;
        Ok::<_, DriverError>(first)
    })();

    assert!(create.unwrap_err().is_unsupported());
    assert_eq!(registrations.get(), 0);
    assert_eq!(retired.load(Ordering::Relaxed), 1);
    drop(context);
    assert_eq!(retired.load(Ordering::Relaxed), 1);
}

#[test]
fn provider_selects_preferred_strategy_from_the_clients_actually_supplied() {
    let preferred = PreferredClient::new();
    let fallback = FallbackClient::new();
    let context = driver_context()
        .with_completion_service(preferred.clone())
        .unwrap()
        .with_completion_service(fallback.clone())
        .unwrap();

    let (context, _driver) = TestContext::provider().unwrap().create(context).unwrap();

    assert_eq!(context.strategy(), Strategy::Preferred);
    assert_eq!(preferred.registrations(), 1);
    assert_eq!(fallback.registrations(), 0);
}

#[test]
fn provider_falls_back_when_the_preferred_client_is_absent() {
    let fallback = FallbackClient::new();
    let context = driver_context().with_completion_service(fallback.clone()).unwrap();

    let (context, _driver) = TestContext::provider().unwrap().create(context).unwrap();

    assert_eq!(context.strategy(), Strategy::Fallback);
    assert_eq!(fallback.registrations(), 1);
}

#[test]
fn unsupported_worker_configuration_fails_before_native_side_effects() {
    let preferred = PreferredClient::new();
    let fallback = FallbackClient::new();
    // A coherent worker configuration that offers neither supported client.
    let context = driver_context().with_completion_service(UnrelatedClient).unwrap();

    let error = TestContext::provider().unwrap().create(context).err().unwrap();

    assert!(error.is_unsupported());
    assert_eq!(preferred.registrations(), 0);
    assert_eq!(fallback.registrations(), 0);
}

#[test]
fn native_registration_failure_during_creation_is_reported_with_its_cause() {
    let preferred = PreferredClient::new();
    let fallback = FallbackClient::new();
    preferred.deny_next_registration();
    let context = driver_context()
        .with_completion_service(preferred.clone())
        .unwrap()
        .with_completion_service(fallback.clone())
        .unwrap();

    let error = TestContext::provider().unwrap().create(context).err().unwrap();

    // A supported strategy that fails natively is a plain failure, not an unsupported worker.
    assert!(!error.is_unsupported());
    assert!(error.source().unwrap().downcast_ref::<io::Error>().is_some());
    assert_eq!(preferred.registrations(), 0);
    assert_eq!(fallback.registrations(), 0);

    let fallback = FallbackClient::new();
    fallback.deny_next_registration();
    let context = driver_context().with_completion_service(fallback.clone()).unwrap();
    let error = TestContext::provider().unwrap().create(context).err().unwrap();
    assert!(!error.is_unsupported());
    assert_eq!(fallback.registrations(), 0);
}

#[test]
fn one_provider_creates_an_independent_instance_per_worker() {
    let provider = TestContext::provider().unwrap();
    let preferred = PreferredClient::new();

    let mut first_clone = provider.clone();
    first_clone.relocate(None, &worker_thread());
    let (first, _first_driver) = first_clone
        .create(driver_context().with_completion_service(preferred.clone()).unwrap())
        .unwrap();

    let mut second_clone = provider;
    second_clone.relocate(None, &worker_thread());
    let (second, _second_driver) = second_clone
        .create(driver_context().with_completion_service(preferred.clone()).unwrap())
        .unwrap();

    // Creation never waits for another worker, and the instances share no driver state.
    assert_eq!(preferred.registrations(), 2);
    first.queue_events(1);
    assert_eq!(first.queued_events(), 1);
    assert_eq!(second.queued_events(), 0);
}

#[test]
fn each_returned_context_belongs_to_its_own_driver_instance() {
    let preferred = PreferredClient::new();
    let provider = TestContext::provider().unwrap();
    let (first, mut first_driver) = provider
        .clone()
        .create(driver_context().with_completion_service(preferred.clone()).unwrap())
        .unwrap();
    let (second, second_driver) = provider
        .create(driver_context().with_completion_service(preferred).unwrap())
        .unwrap();

    // Two instances of the SAME context type: the pairing is a value-level obligation that the
    // tuple does not prove, so it is verified through observable effects.
    let first_operation = first.begin_operation().unwrap();
    let second_operation = second.begin_operation().unwrap();
    first.queue_events(2);
    second.queue_events(3);

    assert_eq!(first_driver.service(&mut budget(MAX_TURNS)).unwrap(), ServiceStatus::Idle);
    assert_eq!(first.completed_events(), 2);
    assert_eq!(
        second.completed_events(),
        0,
        "servicing one driver must not complete the other's work"
    );

    let mut first_drain = first_driver.shutdown();
    assert!(first.is_closed());
    assert!(!second.is_closed(), "shutting down one driver must not close the other's admission");
    let still_admitted = second.begin_operation().unwrap();

    assert_eq!(
        first_drain.service(&mut budget(MAX_TURNS)).unwrap(),
        DrainStatus::Pending(ServiceStatus::Idle),
        "the first drain waits for its own operation, not the other instance's"
    );
    drop(first_operation);
    assert_eq!(first_drain.service(&mut budget(MAX_TURNS)).unwrap(), DrainStatus::Complete);
    assert_eq!(
        second.active_operations(),
        2,
        "the retired instance released none of its peer's state"
    );

    drop(second_driver);
    drop(second_operation);
    drop(still_admitted);
    assert_eq!(second.active_operations(), 0);
    // The runtime owns terminal removal; releasing the completed drain drops only its own driver.
    drop(first_drain);
    assert_eq!(first.drop_calls(), 1);
    assert_eq!(second.drop_calls(), 1);
}

#[test]
fn driver_without_native_clients_installs_on_a_client_free_context() {
    let provider = SoftwareContext::provider().unwrap();
    let (context, mut driver) = provider.create(driver_context()).unwrap();

    context.0.queue_events(1);
    let mut budget = budget(4);
    assert_eq!(driver.service(&mut budget).unwrap(), ServiceStatus::Idle);
    assert_eq!(context.0.completed_events(), 1);
}

#[test]
fn installed_driver_and_drain_stay_local_inline_owners() {
    let preferred = PreferredClient::new();
    let (context, driver): (TestContext, InstalledDriver) = TestContext::provider()
        .unwrap()
        .create(driver_context().with_completion_service(preferred).unwrap())
        .unwrap();

    // Both concrete values are Send and Sync; their installed owners deliberately are not.
    assert!(format!("{driver:?}").contains("LocalDriver"));
    let drain: InstalledDrain = driver.shutdown();
    assert!(format!("{drain:?}").contains("LocalDrain"));
    assert_eq!(context.shutdown_calls(), 1);
    drop(drain);
}

#[test]
fn service_budget_preserves_continuation_without_another_notification() {
    let wake = Arc::new(WakeCount::default());
    let (mut driver, context) = installed_driver_with(&wake);
    context.queue_events(5);

    let mut first_turn = budget(2);
    assert_eq!(driver.service(&mut first_turn).unwrap(), ServiceStatus::Runnable);
    assert_eq!(first_turn.remaining(), 0);
    assert_eq!(context.completed_events(), 2);
    // Continuation is reported by the status, not by a new readiness signal.
    assert_eq!(wake.count(), 0);
    assert_eq!(driver.prepare_wait().unwrap(), WaitStatus::WorkReady);

    let mut second_turn = budget(8);
    assert_eq!(driver.service(&mut second_turn).unwrap(), ServiceStatus::Idle);
    assert_eq!(context.completed_events(), 5);
    assert_eq!(driver.prepare_wait().unwrap(), WaitStatus::Armed);
}

#[test]
fn admission_closes_before_the_first_drain_turn() {
    let (driver, context) = installed_driver();
    let lease = context.begin_operation().unwrap();

    let mut drain = driver.shutdown();

    assert!(context.is_closed());
    assert_eq!(context.shutdown_calls(), 1);
    assert!(context.begin_operation().unwrap_err().to_string().contains("admission is closed"));
    // Admission closure happened before any drain service turn ran.
    assert_eq!(context.drain_turns(), 0);

    let mut budget = budget(4);
    assert_eq!(drain.service(&mut budget).unwrap(), DrainStatus::Pending(ServiceStatus::Idle));
    drop(lease);
    assert_eq!(drain.service(&mut budget).unwrap(), DrainStatus::Complete);
}

#[test]
fn active_operations_not_context_clones_keep_a_drain_pending() {
    let (driver, context) = installed_driver();
    let lease = context.begin_operation().unwrap();
    let retained = context.clone();

    let mut drain = driver.shutdown();
    let mut budget = budget(MAX_TURNS);
    assert_eq!(drain.service(&mut budget).unwrap(), DrainStatus::Pending(ServiceStatus::Idle));

    // A retained context clone is a closed handle; it must not delay retirement.
    assert!(retained.is_closed());
    drop(retained);
    assert_eq!(drain.service(&mut budget).unwrap(), DrainStatus::Pending(ServiceStatus::Idle));

    drop(lease);
    assert_eq!(drain.service(&mut budget).unwrap(), DrainStatus::Complete);
    assert_eq!(context.active_operations(), 0);
}

#[test]
fn independent_drains_interleave_under_the_same_budget_protocol() {
    let (first_driver, first) = installed_driver();
    let (second_driver, second) = installed_driver();
    let mut first_lease = Some(first.begin_operation().unwrap());
    let mut second_lease = Some(second.begin_operation().unwrap());
    first.queue_events(3);
    second.queue_events(1);

    let mut drains = [first_driver.shutdown(), second_driver.shutdown()];
    let mut completed = [false, false];
    let mut turns = 0;
    while !completed.iter().all(|done| *done) {
        turns += 1;
        assert!(turns <= MAX_TURNS, "interleaved drains must retire within a bounded turn count");
        for (index, drain) in drains.iter_mut().enumerate() {
            if completed[index] {
                continue;
            }
            // Each participant gets its own fresh allowance on every turn.
            let mut budget = budget(1);
            match drain.service(&mut budget).unwrap() {
                DrainStatus::Pending(_) => {}
                DrainStatus::Complete => completed[index] = true,
            }
        }
        if turns == 2 {
            drop(first_lease.take());
            drop(second_lease.take());
        }
    }

    // Neither drain monopolized the worker: both retired while sharing one-unit turns.
    assert_eq!(first.completed_events(), 3);
    assert_eq!(second.completed_events(), 1);
    assert!(first.drain_turns() > 1 && second.drain_turns() > 1);
}

#[test]
fn dropping_a_running_driver_closes_admission_without_invalidating_operations() {
    let (driver, context) = installed_driver();
    let lease = context.begin_operation().unwrap();

    drop(driver);

    assert!(context.is_closed());
    assert_eq!(context.shutdown_calls(), 0);
    assert_eq!(context.drop_calls(), 1);
    drop(context.begin_operation().unwrap_err());
    // The admitted operation still owns its state after the driver is gone.
    assert_eq!(context.active_operations(), 1);
    drop(lease);
    assert_eq!(context.active_operations(), 0);
}

#[test]
fn abandoning_a_drain_keeps_admitted_state_owned() {
    let (driver, context) = installed_driver();
    let lease = context.begin_operation().unwrap();

    let drain = driver.shutdown();
    drop(drain);

    assert_eq!(context.drop_calls(), 1);
    assert_eq!(context.active_operations(), 1);
    drop(context.begin_operation().unwrap_err());
    drop(lease);
    assert_eq!(context.active_operations(), 0);
}

#[test]
fn drain_service_failure_preserves_its_native_cause_and_releases_state() {
    let drops = Rc::new(Cell::new(0));
    let mut drain = LocalDriver::new(FailingDrain {
        fail_prepare: false,
        drops: Rc::clone(&drops),
    })
    .shutdown();

    let error = drain.service(&mut budget(4)).unwrap_err();
    assert!(error.source().unwrap().downcast_ref::<io::Error>().is_some());

    // The runtime, not a core wrapper, owns terminal removal after a failure.
    drop(drain);
    assert_eq!(drops.get(), 1);
}

#[test]
fn drain_preparation_failure_is_reported_to_the_runtime() {
    let drops = Rc::new(Cell::new(0));
    let mut drain = LocalDriver::new(FailingDrain {
        fail_prepare: true,
        drops: Rc::clone(&drops),
    })
    .shutdown();

    let error = drain.prepare_wait().unwrap_err();
    assert!(error.to_string().contains("arming"));
    drop(drain);
    assert_eq!(drops.get(), 1);
}

#[test]
fn driver_errors_classify_recovery_without_matching_messages() {
    let error = DriverError::from_message("completion dispatch failed");
    assert!(!error.is_unsupported());
    assert!(!error.is_duplicate_completion_service());
    assert!(!error.is_shutdown_timeout());
    assert_eq!(error.to_string(), "completion dispatch failed");

    let unsupported = DriverError::unsupported("no supported completion client");
    assert!(unsupported.is_unsupported());

    let timeout = DriverError::shutdown_timeout();
    assert!(timeout.is_shutdown_timeout());
    assert!(!timeout.is_unsupported());
}

#[test]
fn classified_errors_retain_a_downcastable_native_cause() {
    let error = DriverError::unsupported("this worker cannot support the driver").with_cause(io::Error::other("native denial"));

    assert!(error.is_unsupported());
    let cause = error.source().unwrap().downcast_ref::<io::Error>().unwrap();
    assert_eq!(cause.to_string(), "native denial");

    let from_cause = DriverError::from_cause(io::Error::other("native failure"));
    assert_eq!(from_cause.to_string(), "native failure");
    assert!(from_cause.source().is_some());
}

#[test]
fn replacing_a_cause_preserves_the_error_classification() {
    let error = DriverError::shutdown_timeout()
        .with_cause(io::Error::other("first"))
        .with_cause(io::Error::other("second"));

    assert!(error.is_shutdown_timeout());
    assert_eq!(error.source().unwrap().to_string(), "second");
}

#[test]
fn nonblocking_collection_never_waits_and_preserves_a_pending_interruption() {
    let mut waiter = TestWaiter::new();
    let waker = waiter.waker();
    waker.wake_by_ref();

    let mut budget = budget(4);
    assert_eq!(waiter.collect(Duration::ZERO, &mut budget).unwrap(), ServiceStatus::Idle);
    assert_eq!(waiter.wait_entries(), 0);
    // The latch still holds the interruption a zero-duration collection must not consume.
    assert!(waiter.is_latched());

    assert_eq!(waiter.collect(Duration::MAX, &mut budget).unwrap(), ServiceStatus::Idle);
    assert_eq!(waiter.wait_entries(), 1);
    assert!(!waiter.is_latched());
}

#[test]
fn exhausted_collection_budget_never_enters_a_wait() {
    let mut waiter = TestWaiter::new();
    waiter.post(0, 1);
    let mut budget = budget(1);
    assert!(budget.try_consume());

    // An unbounded request with no allowance must return without waiting at all.
    assert_eq!(waiter.collect(Duration::MAX, &mut budget).unwrap(), ServiceStatus::Runnable);
    assert_eq!(waiter.wait_entries(), 0);
    assert_eq!(waiter.deliveries(), &[] as &[usize]);
}

#[test]
fn collection_is_fair_across_continuously_actionable_sources() {
    let mut waiter = TestWaiter::new();
    waiter.post(0, MAX_TURNS);
    waiter.post(1, 1);

    let mut budget = budget(2);
    assert_eq!(waiter.collect(Duration::ZERO, &mut budget).unwrap(), ServiceStatus::Runnable);

    // The hot source did not starve its peer within one bounded turn.
    assert_eq!(waiter.deliveries(), &[0, 1]);
}

#[test]
fn collection_preserves_its_cursor_across_bounded_turns() {
    let mut waiter = TestWaiter::new();
    waiter.post(0, 2);
    waiter.post(1, 2);

    let mut first = budget(1);
    assert_eq!(waiter.collect(Duration::ZERO, &mut first).unwrap(), ServiceStatus::Runnable);
    let mut second = budget(1);
    assert_eq!(waiter.collect(Duration::ZERO, &mut second).unwrap(), ServiceStatus::Runnable);

    // Continuation resumes where the exhausted allowance stopped instead of restarting.
    assert_eq!(waiter.deliveries(), &[0, 1]);
}

#[test]
fn collector_waker_stays_safe_after_the_collector_is_destroyed() {
    let waiter = TestWaiter::new();
    let waker = waiter.waker();
    drop(waiter);

    waker.wake_by_ref();
    waker.wake();
}

#[test]
fn a_boxed_collector_attaches_its_own_clients_without_naming_native_types() {
    let waiter: Box<dyn CompletionWaiter> = Box::new(TestWaiter::new());

    let context = waiter.attach_clients(driver_context()).unwrap();
    let (context, _driver) = TestContext::provider().unwrap().create(context).unwrap();
    assert_eq!(context.strategy(), Strategy::Preferred);

    // Attaching the same clients twice is the duplicate-client failure, not a silent replacement.
    let error = waiter.attach_clients(waiter.attach_clients(driver_context()).unwrap()).unwrap_err();
    assert!(error.is_duplicate_completion_service());
}

#[test]
fn a_collector_without_native_clients_uses_the_default_seam() {
    let waiter = ParkingWaiter;
    let mut context = waiter.attach_clients(driver_context()).unwrap();

    assert!(context.take_completion_service::<PreferredClient>().unwrap_err().is_unsupported());
    let (context, _driver) = SoftwareContext::provider().unwrap().create(context).unwrap();
    assert_eq!(context.0.completed_events(), 0);
}

#[test]
fn a_late_readiness_signal_after_destruction_is_safe() {
    let wake = Arc::new(WakeCount::default());
    let preferred = PreferredClient::new();
    let context = DriverContext::new(worker_thread(), system_tasks(), Waker::from(Arc::clone(&wake)))
        .with_completion_service(preferred)
        .unwrap();
    let (context, driver) = TestContext::provider().unwrap().create(context).unwrap();
    let readiness = context.readiness_waker();

    let drain = driver.shutdown();
    drop(drain);

    // A native callback may still hold the handle after the participant is gone.
    readiness.wake_by_ref();
    assert_eq!(wake.count(), 1);
}

// ---------------------------------------------------------------------------------------------
// Test doubles
// ---------------------------------------------------------------------------------------------

struct DualPhase {
    closed: bool,
}

impl Driver for DualPhase {
    type Drain = Self;

    fn service(&mut self, budget: &mut CompletionBudget) -> Result<ServiceStatus, DriverError> {
        assert!(!self.closed);
        Ok(if budget.try_consume() {
            ServiceStatus::Idle
        } else {
            ServiceStatus::Runnable
        })
    }

    fn prepare_wait(&mut self) -> Result<WaitStatus, DriverError> {
        assert!(!self.closed);
        Ok(WaitStatus::Armed)
    }

    fn shutdown(mut self) -> Self::Drain {
        assert!(!self.closed);
        self.closed = true;
        self
    }
}

impl Drain for DualPhase {
    fn service(&mut self, budget: &mut CompletionBudget) -> Result<DrainStatus, DriverError> {
        assert!(self.closed);
        Ok(if budget.try_consume() {
            DrainStatus::Complete
        } else {
            DrainStatus::Pending(ServiceStatus::Runnable)
        })
    }

    fn prepare_wait(&mut self) -> Result<WaitStatus, DriverError> {
        assert!(self.closed);
        Ok(WaitStatus::WorkReady)
    }
}

#[derive(Debug, Default)]
struct WakeCount {
    count: AtomicUsize,
}

impl WakeCount {
    fn count(&self) -> usize {
        // Counts observations only; it does not publish other state.
        self.count.load(Ordering::Relaxed)
    }
}

impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        // The counter is diagnostic and does not synchronize driver state.
        self.count.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Strategy {
    Preferred,
    Fallback,
}

#[derive(Clone, Debug, Default)]
struct PreferredClient {
    registrations: Arc<AtomicUsize>,
    deny: Arc<AtomicBool>,
}

impl PreferredClient {
    fn new() -> Self {
        Self::default()
    }

    /// Makes the next native registration fail, the way a real adapter reports a denial.
    fn deny_next_registration(&self) {
        self.deny.store(true, Ordering::Relaxed);
    }

    fn register(&self, _readiness: &Waker) -> Result<(), DriverError> {
        if self.deny.swap(false, Ordering::Relaxed) {
            return Err(DriverError::from_cause(io::Error::other("native registration denied")));
        }
        self.registrations.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn registrations(&self) -> usize {
        self.registrations.load(Ordering::Relaxed)
    }
}

#[derive(Clone, Debug, Default)]
struct FallbackClient {
    registrations: Arc<AtomicUsize>,
    deny: Arc<AtomicBool>,
}

impl FallbackClient {
    fn new() -> Self {
        Self::default()
    }

    /// Makes the next native registration fail, the way a real adapter reports a denial.
    fn deny_next_registration(&self) {
        self.deny.store(true, Ordering::Relaxed);
    }

    fn register(&self, _readiness: &Waker) -> Result<(), DriverError> {
        if self.deny.swap(false, Ordering::Relaxed) {
            return Err(DriverError::from_cause(io::Error::other("native registration denied")));
        }
        self.registrations.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn registrations(&self) -> usize {
        self.registrations.load(Ordering::Relaxed)
    }
}

/// A client type this driver does not understand, proving lookup is by exact type identity.
#[derive(Clone, Debug)]
struct UnrelatedClient;

/// A move-only, thread-affine native lease: no `Clone`, no `Send`/`Sync`. Ownership-capable
/// extraction must work for this shape without an artificial `Clone` or `RefCell` workaround.
#[derive(Debug)]
struct RecordLease {
    retired: Arc<AtomicUsize>,
    owner: thread::ThreadId,
    _not_send: PhantomData<*const ()>,
}

impl RecordLease {
    fn new(retired: Arc<AtomicUsize>) -> Self {
        Self {
            retired,
            owner: thread::current().id(),
            _not_send: PhantomData,
        }
    }
}

impl Drop for RecordLease {
    fn drop(&mut self) {
        assert_eq!(thread::current().id(), self.owner);
        self.retired.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Debug, Default)]
struct State {
    closed: bool,
    queued_events: usize,
    completed_events: usize,
    active: usize,
    shutdown_calls: usize,
    drop_calls: usize,
    drain_turns: usize,
}

#[derive(Clone, Debug)]
struct TestContext {
    state: Arc<Mutex<State>>,
    strategy: Strategy,
    readiness: Waker,
}

impl TestContext {
    fn fresh(strategy: Strategy, readiness: Waker) -> Self {
        Self {
            state: Arc::new(Mutex::new(State::default())),
            strategy,
            readiness,
        }
    }

    fn strategy(&self) -> Strategy {
        self.strategy
    }

    fn begin_operation(&self) -> Result<OperationLease, DriverError> {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return Err(DriverError::from_message("admission is closed"));
        }
        state.active += 1;
        Ok(OperationLease {
            state: Arc::clone(&self.state),
        })
    }

    fn queue_events(&self, count: usize) {
        self.state.lock().unwrap().queued_events += count;
    }

    fn queued_events(&self) -> usize {
        self.state.lock().unwrap().queued_events
    }

    fn completed_events(&self) -> usize {
        self.state.lock().unwrap().completed_events
    }

    fn active_operations(&self) -> usize {
        self.state.lock().unwrap().active
    }

    fn is_closed(&self) -> bool {
        self.state.lock().unwrap().closed
    }

    fn shutdown_calls(&self) -> usize {
        self.state.lock().unwrap().shutdown_calls
    }

    fn drop_calls(&self) -> usize {
        self.state.lock().unwrap().drop_calls
    }

    fn drain_turns(&self) -> usize {
        self.state.lock().unwrap().drain_turns
    }

    fn readiness_waker(&self) -> Waker {
        self.readiness.clone()
    }
}

impl ThreadAware for TestContext {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl IoContext for TestContext {
    type Provider = TestProvider;

    fn provider() -> Result<Self::Provider, DriverError> {
        Ok(TestProvider)
    }
}

#[derive(Debug)]
struct OperationLease {
    state: Arc<Mutex<State>>,
}

impl Drop for OperationLease {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap();
        state.active = state.active.saturating_sub(1);
    }
}

#[derive(Clone, Debug)]
struct TestProvider;

impl ThreadAware for TestProvider {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl DriverProvider for TestProvider {
    type Context = TestContext;
    type Driver = TestDriver;

    fn create(self, mut context: DriverContext) -> Result<(Self::Context, LocalDriver<Self::Driver>), DriverError> {
        let readiness = context.readiness_waker().clone();
        // Select from the client this worker actually supplies, before any native side effect.
        // Fallback follows only a missing extraction, never a native registration failure: once
        // a client is taken, its `?` below reports that failure directly.
        let strategy = if let Ok(client) = context.take_completion_service::<PreferredClient>() {
            client.register(&readiness)?;
            Strategy::Preferred
        } else if let Ok(client) = context.take_completion_service::<FallbackClient>() {
            client.register(&readiness)?;
            Strategy::Fallback
        } else {
            return Err(DriverError::unsupported(
                "this worker supplies no completion client this driver supports",
            ));
        };
        // The published handle and the installed driver share one newly created state.
        let handle = TestContext::fresh(strategy, readiness);
        let driver = TestDriver { context: handle.clone() };
        Ok((handle, LocalDriver::new(driver)))
    }
}

#[derive(Debug)]
struct TestDriver {
    context: TestContext,
}

impl TestDriver {
    fn close(&self) {
        self.context.state.lock().unwrap().closed = true;
    }
}

impl Drop for TestDriver {
    fn drop(&mut self) {
        let mut state = self.context.state.lock().unwrap();
        state.closed = true;
        state.drop_calls += 1;
    }
}

impl Driver for TestDriver {
    type Drain = TestDrain;

    fn service(&mut self, budget: &mut CompletionBudget) -> Result<ServiceStatus, DriverError> {
        let mut state = self.context.state.lock().unwrap();
        while state.queued_events > 0 {
            if !budget.try_consume() {
                return Ok(ServiceStatus::Runnable);
            }
            state.queued_events -= 1;
            state.completed_events += 1;
        }
        Ok(ServiceStatus::Idle)
    }

    fn prepare_wait(&mut self) -> Result<WaitStatus, DriverError> {
        Ok(if self.context.state.lock().unwrap().queued_events == 0 {
            WaitStatus::Armed
        } else {
            WaitStatus::WorkReady
        })
    }

    fn shutdown(self) -> Self::Drain {
        // Admission closes synchronously, before the runtime can service the returned drain.
        self.close();
        self.context.state.lock().unwrap().shutdown_calls += 1;
        TestDrain { driver: self }
    }
}

struct TestDrain {
    driver: TestDriver,
}

impl Drain for TestDrain {
    fn service(&mut self, budget: &mut CompletionBudget) -> Result<DrainStatus, DriverError> {
        let status = self.driver.service(budget)?;
        let mut state = self.driver.context.state.lock().unwrap();
        state.drain_turns += 1;
        if state.active == 0 && state.queued_events == 0 {
            return Ok(DrainStatus::Complete);
        }
        drop(state);
        Ok(DrainStatus::Pending(status))
    }

    fn prepare_wait(&mut self) -> Result<WaitStatus, DriverError> {
        self.driver.prepare_wait()
    }
}

/// A driver that needs no native client capability at all.
#[derive(Clone, Debug)]
struct SoftwareContext(TestContext);

impl ThreadAware for SoftwareContext {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl IoContext for SoftwareContext {
    type Provider = SoftwareProvider;

    fn provider() -> Result<Self::Provider, DriverError> {
        Ok(SoftwareProvider)
    }
}

#[derive(Clone, Debug)]
struct SoftwareProvider;

impl ThreadAware for SoftwareProvider {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl DriverProvider for SoftwareProvider {
    type Context = SoftwareContext;
    type Driver = TestDriver;

    fn create(self, context: DriverContext) -> Result<(Self::Context, LocalDriver<Self::Driver>), DriverError> {
        // A driver names no context family, so one implementation serves a distinct context type
        // without a delegating wrapper.
        let handle = TestContext::fresh(Strategy::Fallback, context.readiness_waker().clone());
        let driver = TestDriver { context: handle.clone() };
        Ok((SoftwareContext(handle), LocalDriver::new(driver)))
    }
}

struct FailingDrain {
    fail_prepare: bool,
    drops: Rc<Cell<usize>>,
}

impl Driver for FailingDrain {
    type Drain = Self;

    fn service(&mut self, _budget: &mut CompletionBudget) -> Result<ServiceStatus, DriverError> {
        Ok(ServiceStatus::Idle)
    }

    fn prepare_wait(&mut self) -> Result<WaitStatus, DriverError> {
        Ok(WaitStatus::Armed)
    }

    fn shutdown(self) -> Self::Drain {
        self
    }
}

impl Drain for FailingDrain {
    fn service(&mut self, _budget: &mut CompletionBudget) -> Result<DrainStatus, DriverError> {
        Err(DriverError::from_cause(io::Error::other("native completion failed")))
    }

    fn prepare_wait(&mut self) -> Result<WaitStatus, DriverError> {
        if self.fail_prepare {
            Err(DriverError::from_message("arming the drain failed"))
        } else {
            Ok(WaitStatus::Armed)
        }
    }
}

impl Drop for FailingDrain {
    fn drop(&mut self) {
        self.drops.update(|count| count + 1);
    }
}

#[derive(Debug, Default)]
struct Latch {
    raised: Mutex<bool>,
    changed: Condvar,
}

impl Latch {
    fn is_raised(&self) -> bool {
        *self.raised.lock().unwrap()
    }

    /// Waits at most `max_wait`, and never longer than [`WAIT_CAP`] even for an unbounded request.
    fn wait(&self, max_wait: Duration) {
        let mut raised = self.raised.lock().unwrap();
        if !*raised {
            (raised, _) = self
                .changed
                .wait_timeout_while(raised, max_wait.min(WAIT_CAP), |raised| !*raised)
                .unwrap();
        }
        *raised = false;
    }
}

impl Wake for Latch {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        *self.raised.lock().unwrap() = true;
        self.changed.notify_one();
    }
}

/// An in-memory collector with two independent native sources and a fair delivery cursor.
struct TestWaiter {
    latch: Arc<Latch>,
    pending: [usize; 2],
    cursor: usize,
    deliveries: Vec<usize>,
    wait_entries: usize,
    preferred: PreferredClient,
    fallback: FallbackClient,
}

impl TestWaiter {
    fn new() -> Self {
        Self {
            latch: Arc::default(),
            pending: [0; 2],
            cursor: 0,
            deliveries: Vec::new(),
            wait_entries: 0,
            preferred: PreferredClient::new(),
            fallback: FallbackClient::new(),
        }
    }

    fn post(&mut self, source: usize, count: usize) {
        self.pending[source] += count;
    }

    fn deliveries(&self) -> &[usize] {
        &self.deliveries
    }

    fn wait_entries(&self) -> usize {
        self.wait_entries
    }

    fn is_latched(&self) -> bool {
        self.latch.is_raised()
    }

    fn status(&self) -> ServiceStatus {
        if self.pending.iter().any(|count| *count > 0) {
            ServiceStatus::Runnable
        } else {
            ServiceStatus::Idle
        }
    }
}

impl CompletionWaiter for TestWaiter {
    fn waker(&self) -> Waker {
        Waker::from(Arc::clone(&self.latch))
    }

    fn collect(&mut self, max_wait: Duration, budget: &mut CompletionBudget) -> Result<ServiceStatus, DriverError> {
        // An exhausted allowance never enters a wait, whatever duration was requested.
        if budget.remaining() == 0 {
            return Ok(self.status());
        }
        if !max_wait.is_zero() {
            self.wait_entries += 1;
            self.latch.wait(max_wait);
        }
        while let Some(offset) = (0..self.pending.len()).find(|offset| self.pending[(self.cursor + offset) % 2] > 0) {
            // Charge before the delivery step, never for an absent one.
            if !budget.try_consume() {
                break;
            }
            let source = (self.cursor + offset) % 2;
            self.pending[source] -= 1;
            self.deliveries.push(source);
            // Advance past the source just served so a hot peer cannot starve the others.
            self.cursor = (source + 1) % 2;
        }
        Ok(self.status())
    }

    fn attach_clients(&self, context: DriverContext) -> Result<DriverContext, DriverError> {
        let context = context.with_completion_service(self.preferred.clone())?;
        context.with_completion_service(self.fallback.clone())
    }
}

/// A runtime with no native sources still satisfies the collector contract.
struct ParkingWaiter;

impl CompletionWaiter for ParkingWaiter {
    fn waker(&self) -> Waker {
        Waker::noop().clone()
    }

    fn collect(&mut self, _max_wait: Duration, _budget: &mut CompletionBudget) -> Result<ServiceStatus, DriverError> {
        Ok(ServiceStatus::Idle)
    }
}

fn budget(units: usize) -> CompletionBudget {
    CompletionBudget::new(NonZeroUsize::new(units).unwrap())
}

fn system_tasks() -> SystemTasks {
    SystemTasks::new(|task| {
        task.run();
        Ok(())
    })
}

fn driver_context() -> DriverContext {
    DriverContext::new(worker_thread(), system_tasks(), Waker::noop().clone())
}

fn installed_driver() -> (InstalledDriver, TestContext) {
    installed_driver_with(&Arc::new(WakeCount::default()))
}

fn installed_driver_with(wake: &Arc<WakeCount>) -> (InstalledDriver, TestContext) {
    let context = DriverContext::new(worker_thread(), system_tasks(), Waker::from(Arc::clone(wake)))
        .with_completion_service(PreferredClient::new())
        .unwrap();
    let (handle, driver) = TestContext::provider().unwrap().create(context).unwrap();
    (driver, handle)
}

fn worker_thread() -> Thread {
    let owner = thread_aware_core::__private::v1::new_owner();
    let numa_node = thread_aware_core::__private::v1::new_numa_node(0);
    thread_aware_core::__private::v1::new_thread(owner, thread::current().id(), numa_node)
}
