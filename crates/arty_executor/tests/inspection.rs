// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(feature = "test-util")]

//! Inspections read native storage without changing scheduling or extending task lifetimes.

testing_aids::init_tracing!();

use std::cell::{Cell, RefCell};
use std::future::{pending, poll_fn, ready};
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::task::{Poll, Waker};

use arty_executor::testing::new_guarded_executor;
use arty_executor::{InspectionError, InspectionStatus, PollObserver, TaskState};

#[test]
fn descriptions_are_lazy_bounded_and_exclude_completed_tasks() {
    let executor = new_guarded_executor(Waker::noop().clone());
    let tasks = executor.tasks();
    let calls = Rc::new(Cell::new(0));
    let counter = Rc::clone(&calls);
    let completed = tasks.add_with_inspection(ready(()), move |_, state| {
        counter.set(counter.get() + 1);
        Some((1_u32, state))
    });
    let waiting = tasks.add_with_inspection(pending::<()>(), |_, state| Some((2_u32, state)));
    assert_eq!(calls.get(), 0);

    let mut descriptions = Vec::new();
    let status = tasks
        .inspect(NonZeroUsize::new(1).unwrap(), |description: &(u32, TaskState)| {
            descriptions.push(*description);
        })
        .unwrap();
    assert_eq!((status, descriptions), (InspectionStatus::Truncated, vec![(1, TaskState::Ready)]));

    let _ = executor.execute_cycle();
    let mut descriptions = Vec::new();
    let status = tasks
        .inspect(NonZeroUsize::new(10).unwrap(), |description: &(u32, TaskState)| {
            descriptions.push(*description);
        })
        .unwrap();
    assert_eq!((status, descriptions), (InspectionStatus::Complete, vec![(2, TaskState::Waiting)]));
    drop((completed, waiting));
}

#[test]
fn inspecting_a_notified_task_does_not_consume_its_wake() {
    let executor = new_guarded_executor(Waker::noop().clone());
    let tasks = executor.tasks();
    let done = Rc::new(Cell::new(false));
    let waker = Rc::new(RefCell::new(None::<Waker>));
    let future_done = Rc::clone(&done);
    let future_waker = Rc::clone(&waker);
    let task = tasks.add_with_inspection(
        poll_fn(move |context| {
            if future_done.get() {
                Poll::Ready(())
            } else {
                *future_waker.borrow_mut() = Some(context.waker().clone());
                Poll::Pending
            }
        }),
        |_, state| Some(state),
    );
    let _ = executor.execute_cycle();
    done.set(true);
    waker.borrow().as_ref().unwrap().wake_by_ref();
    let mut descriptions = Vec::new();
    tasks
        .inspect(NonZeroUsize::new(10).unwrap(), |state: &TaskState| descriptions.push(*state))
        .unwrap();
    assert_eq!(descriptions, [TaskState::Ready]);

    let _ = executor.execute_cycle();
    let mut descriptions = Vec::new();
    tasks
        .inspect(NonZeroUsize::new(10).unwrap(), |state: &TaskState| descriptions.push(*state))
        .unwrap();
    assert!(descriptions.is_empty());
    waker.borrow_mut().take();
    drop(task);
}

#[test]
fn inspection_during_poll_is_rejected_without_panicking() {
    let executor = new_guarded_executor(Waker::noop().clone());
    let tasks = executor.tasks();
    let nested = tasks.clone();
    let result = Rc::new(Cell::new(None));
    let observed = Rc::clone(&result);
    let task = tasks.add(poll_fn(move |_| {
        observed.set(Some(nested.inspect::<()>(NonZeroUsize::new(1).unwrap(), |()| {}).unwrap_err()));
        Poll::Ready(())
    }));
    let _ = executor.execute_cycle();
    assert_eq!(result.get(), Some(InspectionError::Busy));
    drop(task);
}

#[test]
fn inspection_after_executor_drop_reports_disconnection() {
    let executor = new_guarded_executor(Waker::noop().clone());
    let tasks = executor.tasks();
    drop(executor);
    assert_eq!(
        tasks.inspect::<()>(NonZeroUsize::new(1).unwrap(), |()| {}),
        Err(InspectionError::Disconnected)
    );
}

#[test]
fn a_description_panic_does_not_stop_the_executor() {
    let executor = new_guarded_executor(Waker::noop().clone());
    let tasks = executor.tasks();
    let task = tasks.add_with_inspection(ready(()), |_, _| -> Option<()> { panic!("description") });
    assert_eq!(
        tasks.inspect::<()>(NonZeroUsize::new(10).unwrap(), |()| {}),
        Err(InspectionError::Panicked)
    );
    let _ = executor.execute_cycle();
    assert_eq!(
        tasks.inspect::<()>(NonZeroUsize::new(10).unwrap(), |()| {}),
        Ok(InspectionStatus::Complete)
    );
    drop(task);
}

#[derive(Debug)]
struct RequestedObserver {
    enabled: Rc<Cell<bool>>,
    descriptions: Rc<Cell<usize>>,
    completions: Rc<Cell<usize>>,
}

impl PollObserver for RequestedObserver {
    fn enabled(&self) -> bool {
        self.enabled.get()
    }
    fn poll_started(&self, description: &dyn std::any::Any) {
        assert_eq!(description.downcast_ref::<TaskState>(), Some(&TaskState::Running));
        self.descriptions.set(self.descriptions.get() + 1);
    }
    fn poll_finished(&self, completed: bool) {
        self.completions.set(self.completions.get() + usize::from(completed));
    }
}

#[test]
fn disabled_observers_do_not_describe_tasks_and_enable_at_a_cycle_boundary() {
    let executor = new_guarded_executor(Waker::noop().clone());
    let tasks = executor.tasks();
    let enabled = Rc::new(Cell::new(false));
    let descriptions = Rc::new(Cell::new(0));
    let completions = Rc::new(Cell::new(0));
    let produced = Rc::new(Cell::new(0));
    tasks
        .set_poll_observer(Rc::new(RequestedObserver {
            enabled: Rc::clone(&enabled),
            descriptions: Rc::clone(&descriptions),
            completions: Rc::clone(&completions),
        }))
        .unwrap();
    for requested in [false, true, false] {
        enabled.set(requested);
        let observed = Rc::clone(&produced);
        let task = tasks.add_with_inspection(ready(()), move |_, state| {
            observed.set(observed.get() + 1);
            Some(state)
        });
        let _ = executor.execute_cycle();
        drop(task);
    }

    assert_eq!((produced.get(), descriptions.get(), completions.get()), (1, 1, 1));
}

#[test]
fn enabled_observers_only_finish_polls_that_produced_a_description() {
    let executor = new_guarded_executor(Waker::noop().clone());
    let tasks = executor.tasks();
    let descriptions = Rc::new(Cell::new(0));
    let completions = Rc::new(Cell::new(0));
    tasks
        .set_poll_observer(Rc::new(RequestedObserver {
            enabled: Rc::new(Cell::new(true)),
            descriptions: Rc::clone(&descriptions),
            completions: Rc::clone(&completions),
        }))
        .unwrap();
    let ordinary = tasks.add(ready(()));
    let omitted = tasks.add_with_inspection(ready(()), |_, _| None::<TaskState>);
    let described = tasks.add_with_inspection(ready(()), |_, state| Some(state));
    let _ = executor.execute_cycle();
    assert_eq!((descriptions.get(), completions.get()), (1, 1));
    drop((ordinary, omitted, described));
}
