// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Direct task observation and Seismograph monitoring contracts.

use std::cell::{Cell, RefCell};
use std::future::pending;
use std::rc::Rc;
use std::sync::{Arc, Mutex, mpsc};
use std::task::{Poll, Wake, Waker};

use arty_executor::{Executor, PanicPolicy, ShutdownCause, TaskCompletion, TaskMetadata, TaskObserver, TaskPanicPayload};
use seismograph::recorder::runtime::TypeDescriptorId;
use seismograph_runtime::snapshot::{CaptureOptions, TaskActivityState, TaskCollection};
use seismograph_runtime::worker::WorkerHandle;
use testing_aids::{TEST_TIMEOUT, is_mutation_testing};

static MONITORING: Mutex<()> = Mutex::new(());

#[derive(Debug)]
struct Completion;

impl TaskCompletion<()> for Completion {
    fn completed(self, (): ()) {}

    fn panicked(self, panic: TaskPanicPayload, _cause: Option<ShutdownCause>) {
        arty_executor::discard_task_panic_payload(panic);
    }

    fn cancelled(self, _cause: ShutdownCause) {}
}

#[derive(Debug)]
struct CaptureCompletion(Rc<RefCell<Vec<Option<seismograph::recorder::runtime::TaskId>>>>);

impl TaskCompletion<()> for CaptureCompletion {
    fn completed(self, (): ()) {
        self.0.borrow_mut().push(arty_executor::current_task());
    }

    fn panicked(self, panic: TaskPanicPayload, _cause: Option<ShutdownCause>) {
        self.0.borrow_mut().push(arty_executor::current_task());
        arty_executor::discard_task_panic_payload(panic);
    }

    fn cancelled(self, _cause: ShutdownCause) {}
}

#[derive(Debug)]
struct Notify(mpsc::Sender<()>);

impl Wake for Notify {
    #[expect(clippy::unwrap_used, reason = "the test wake receiver remains connected")]
    fn wake(self: Arc<Self>) {
        self.0.send(()).unwrap();
    }

    #[expect(clippy::unwrap_used, reason = "the test wake receiver remains connected")]
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.send(()).unwrap();
    }
}

struct ResetRecorder;

impl Drop for ResetRecorder {
    fn drop(&mut self) {
        seismograph::recorder(seismograph::recorder::Configuration::default());
    }
}

#[expect(clippy::unwrap_used, reason = "test channel failures require the receiver error")]
fn receive<T>(receiver: &mpsc::Receiver<T>) -> T {
    if is_mutation_testing() {
        receiver.recv().unwrap()
    } else {
        receiver.recv_timeout(TEST_TIMEOUT.min(std::time::Duration::from_secs(5))).unwrap()
    }
}

fn task_metadata<T: 'static>(worker: WorkerHandle) -> Option<TaskMetadata> {
    Some(TaskMetadata::capture::<T>(worker.task_events(), None, size_of::<T>()))
}

#[derive(Debug)]
struct Observer {
    starts: Rc<Cell<usize>>,
    ends: Rc<Cell<usize>>,
}

impl TaskObserver for Observer {
    fn poll_start(&mut self) {
        self.starts.set(self.starts.get() + 1);
    }

    fn poll_end(&mut self, _outcome: Poll<()>) {
        self.ends.set(self.ends.get() + 1);
    }
}

#[test]
fn task_observer_receives_paired_poll_callbacks() {
    let _monitoring = MONITORING.lock().unwrap();
    let starts = Rc::new(Cell::new(0));
    let ends = Rc::new(Cell::new(0));
    let mut executor = Executor::builder().build();
    executor
        .tasks()
        .task(async {})
        .observe(Observer {
            starts: Rc::clone(&starts),
            ends: Rc::clone(&ends),
        })
        .register(Completion);

    let _ = executor.execute_cycle();

    assert_eq!((starts.get(), ends.get()), (1, 1));
}

#[test]
fn terminal_callbacks_run_outside_the_finished_task_context() {
    let _monitoring = MONITORING.lock().unwrap();
    let _reset = ResetRecorder;
    seismograph::recorder(seismograph::recorder::Configuration {
        runtime_tasks: seismograph::recorder::Policy::all(false),
        ..seismograph::recorder::Configuration::default()
    });
    let runtime = seismograph_runtime::register_runtime(seismograph_runtime::RuntimeMetadata::new("terminal-context", 1));
    let worker = runtime.register_worker(seismograph_runtime::worker::WorkerMetadata::new(
        seismograph_runtime::worker::WorkerRole::Core,
    ));
    let mut executor = Executor::builder()
        .panic_policy(PanicPolicy::IGNORE)
        .monitor(worker.handle())
        .build();
    let observed = Rc::new(RefCell::new(Vec::new()));
    executor
        .tasks()
        .task(async {})
        .monitor(task_metadata::<()>(worker.handle()))
        .register(CaptureCompletion(Rc::clone(&observed)));
    executor
        .tasks()
        .task(async { panic!("task panic") })
        .monitor(task_metadata::<()>(worker.handle()))
        .register(CaptureCompletion(Rc::clone(&observed)));

    let _ = executor.execute_cycle();

    assert_eq!(&*observed.borrow(), &[None, None]);
}

#[test]
fn direct_monitoring_is_active_only_for_monitored_cycles() {
    let _monitoring = MONITORING.lock().unwrap();
    let runtime = seismograph_runtime::register_runtime(seismograph_runtime::RuntimeMetadata::new("direct-executor", 1));
    let worker = runtime.register_worker(seismograph_runtime::worker::WorkerMetadata::new(
        seismograph_runtime::worker::WorkerRole::Core,
    ));
    let handle = worker.handle();
    let mut executor = Executor::builder().monitor(handle).build();
    let polls = Rc::new(RefCell::new(Vec::new()));
    let observed = Rc::clone(&polls);
    let mut first = true;
    executor
        .tasks()
        .task(std::future::poll_fn(move |cx| {
            observed.borrow_mut().push(arty_executor::current_task());
            if first {
                first = false;
                cx.waker().wake_by_ref();
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        }))
        .register(Completion);

    let _ = executor.execute_cycle();
    let _reset = ResetRecorder;
    seismograph::recorder(seismograph::recorder::Configuration {
        runtime_tasks: seismograph::recorder::Policy::all(false),
        ..seismograph::recorder::Configuration::default()
    });
    let _ = executor.execute_cycle();

    let polls = polls.borrow();
    assert_eq!(polls.len(), 2);
    assert_eq!(polls[0], None);
    assert!(polls[1].is_some());
}

#[test]
fn canceled_capture_does_not_observe_the_next_poll() {
    let _monitoring = MONITORING.lock().unwrap();
    let _reset = ResetRecorder;
    seismograph::recorder(seismograph::recorder::Configuration {
        runtime_tasks: seismograph::recorder::Policy::all(false),
        ..seismograph::recorder::Configuration::default()
    });
    let runtime = seismograph_runtime::register_runtime(seismograph_runtime::RuntimeMetadata::new("canceled-capture", 1));
    let worker = runtime.register_worker(seismograph_runtime::worker::WorkerMetadata::new(
        seismograph_runtime::worker::WorkerRole::Core,
    ));
    let (wakes, wake_receiver) = mpsc::channel();
    let (requests, request_receiver) = mpsc::channel();
    let request_wakes = wakes.clone();
    worker
        .handle()
        .register_task_provider(move |request| {
            requests.send(request).unwrap();
            request_wakes.send(()).unwrap();
        })
        .unwrap();
    let mut executor = Executor::builder()
        .owner_waker(Waker::from(Arc::new(Notify(wakes))))
        .monitor(worker.handle())
        .build();
    let observed = Rc::new(Cell::new(None));
    let polled = Rc::clone(&observed);
    executor
        .tasks()
        .task(std::future::poll_fn(move |_| {
            polled.set(arty_executor::current_task());
            Poll::Ready(())
        }))
        .monitor(task_metadata::<()>(worker.handle()))
        .register(Completion);
    seismograph::recorder(seismograph::recorder::Configuration::default());

    let capture = std::thread::spawn(move || {
        runtime
            .handle()
            .capture_with_timeout(seismograph_runtime::snapshot::CaptureOptions::default())
            .unwrap()
    });
    receive(&wake_receiver);
    drop(receive(&request_receiver));
    let _ = capture.join().unwrap();
    let _ = executor.execute_cycle();

    assert_eq!(observed.get(), None);
}

#[test]
fn capture_limit_includes_unrecorded_tasks() {
    let _monitoring = MONITORING.lock().unwrap();
    let _reset = ResetRecorder;
    seismograph::recorder(seismograph::recorder::Configuration {
        runtime_tasks: seismograph::recorder::Policy::all(false),
        ..seismograph::recorder::Configuration::default()
    });
    let name = "monitoring-work-limit";
    let runtime = seismograph_runtime::register_runtime(seismograph_runtime::RuntimeMetadata::new(name, 1));
    let worker = runtime.register_worker(seismograph_runtime::worker::WorkerMetadata::new(
        seismograph_runtime::worker::WorkerRole::Core,
    ));
    let (wakes, wake_receiver) = mpsc::channel();
    let (requests, request_receiver) = mpsc::channel();
    let request_wakes = wakes.clone();
    worker
        .handle()
        .register_task_provider(move |request| {
            requests.send(request).unwrap();
            request_wakes.send(()).unwrap();
        })
        .unwrap();
    let executor = Executor::builder()
        .owner_waker(Waker::from(Arc::new(Notify(wakes))))
        .monitor(worker.handle())
        .build();
    executor.tasks().task(pending::<()>()).register(Completion);
    executor
        .tasks()
        .task(pending::<()>())
        .monitor(task_metadata::<()>(worker.handle()))
        .register(Completion);
    let capture = std::thread::spawn(move || {
        runtime
            .handle()
            .capture(CaptureOptions {
                limit_per_worker: 1,
                timeout: TEST_TIMEOUT.min(std::time::Duration::from_secs(5)),
                ..CaptureOptions::default()
            })
            .unwrap()
    });

    receive(&wake_receiver);
    let request = receive(&request_receiver);
    let mut tasks = Vec::new();
    let truncated = executor.inspect_tasks(request.limit(), |task| tasks.push(task));
    request.respond(
        tasks,
        if truncated {
            TaskCollection::Truncated
        } else {
            TaskCollection::Complete
        },
    );
    let captured = capture.join().unwrap();

    let [task] = captured.tasks.as_slice() else {
        panic!("expected exactly one task, got {:?}", captured.tasks);
    };
    assert_eq!(task.activity.unwrap().state, TaskActivityState::Ready);
    assert_eq!(task.type_descriptor, TypeDescriptorId::unknown());
    assert_eq!((task.parent, task.spawned_at, task.future_size_bytes), (None, None, None));
    assert_eq!(captured.workers[0].task_collection, TaskCollection::Truncated);
}
