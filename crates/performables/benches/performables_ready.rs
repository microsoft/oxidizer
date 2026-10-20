// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Ready, uncontended primitive comparisons with runtime recording disabled.
//!
//! Construction, queue warm-up and oneshot delivery are outside measurement.
//! Async cases poll already-ready futures without entering an executor.
//! Arc cases include both clone and non-final drop in the measured function.
//! Queue round trips measure one send and one receive with spare capacity.
//!
//! Tokio queues have one receiver, unlike Performables' MPMC queues. Tokio's
//! Notify retains a permit, unlike a condition variable; its empty notification
//! case measures coalesced notifications. Barrier cases use one participant and
//! do not establish multi-participant scalability. Instruction counts for native
//! notifications do not measure the underlying operating-system wake cost.

#![allow(missing_docs, reason = "benchmark code")]
#![expect(
    clippy::exit,
    clippy::missing_docs_in_private_items,
    unused_qualifications,
    reason = "Triggered by Gungraun macro expansion on every platform."
)]

use std::hint::black_box;
use std::pin::pin;
use std::sync::mpsc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use criterion::{BatchSize, Criterion};
use performables::arc::Arc;
use performables::sync::Async;
use performables::sync::barrier::Barrier;
use performables::sync::channel::{self, OneshotReceiver, Receiver, Sender, WatchReceiver, WatchSender};
use performables::sync::condition::Condvar;
use performables::sync::notify::Notify;
use performables::sync::once::{LazyLock, OnceLock};

const ARC_CYCLE: &str = "performables_ready/arc_clone_drop";
const BARRIER: &str = "performables_ready/barrier";
const ONCE: &str = "performables_ready/once_initialized";
const LAZY: &str = "performables_ready/lazy_initialized";
const NOTIFY: &str = "performables_ready/notify_empty";
const QUEUE_TRY: &str = "performables_ready/bounded_try";
const QUEUE_BLOCKING: &str = "performables_ready/bounded_blocking";
const QUEUE_ASYNC: &str = "performables_ready/bounded_async";
const UNBOUNDED: &str = "performables_ready/unbounded_try";
const WATCH_READ: &str = "performables_ready/watch_borrow";
const WATCH_WRITE: &str = "performables_ready/watch_replace";
const ONESHOT: &str = "performables_ready/oneshot_ready";
const PERMIT_COALESCED: &str = "performables_ready/notify_coalesced";
const PERMIT_CYCLE: &str = "performables_ready/notify_cycle";
const QUEUE_CAPACITY: usize = 64;
const SEND_READY: &str = "the queue has spare capacity and a live receiver";
const RECEIVE_READY: &str = "the preceding send queued one value";
const READY_BLOCKING_SINGLE: &str = "performables_ready/ready_blocking";
const READY_BATCH: BatchSize = BatchSize::NumIterations(64);

type Queue = (Sender<u64>, Receiver<u64>);

fn empty_blocking_queue(bounded: bool) -> Queue {
    let queue = if bounded { channel::bounded(1) } else { channel::unbounded() };
    queue.0.try_send(7).expect(SEND_READY);
    queue.1.try_recv().expect(RECEIVE_READY);
    queue
}

fn ready_blocking_queue(bounded: bool) -> Queue {
    let queue = empty_blocking_queue(bounded);
    queue.0.try_send(7).expect(SEND_READY);
    queue
}

#[metabench::benchmark(SINGLE_QUEUE_SEND, READY_BLOCKING_SINGLE, "queue_send")]
#[bench::bounded(&mut empty_blocking_queue(true))]
#[bench::unbounded(&mut empty_blocking_queue(false))]
fn single_queue_send(queue: &mut Queue) {
    black_box(&queue.0).send(black_box(7)).expect(SEND_READY);
}

#[metabench::benchmark(SINGLE_QUEUE_RECV, READY_BLOCKING_SINGLE, "queue_recv")]
#[bench::bounded(&mut ready_blocking_queue(true))]
#[bench::unbounded(&mut ready_blocking_queue(false))]
fn single_queue_recv(queue: &mut Queue) -> u64 {
    black_box(&queue.1).recv().expect(RECEIVE_READY)
}

#[metabench::benchmark(SINGLE_QUEUE_TIMEOUT, READY_BLOCKING_SINGLE, "queue_recv_timeout")]
#[bench::bounded(&mut ready_blocking_queue(true))]
#[bench::unbounded(&mut ready_blocking_queue(false))]
fn single_queue_timeout(queue: &mut Queue) -> u64 {
    black_box(&queue.1).recv_timeout(Duration::ZERO).expect(RECEIVE_READY)
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "each batched measurement takes one prepared receiver through its Option"
)]
fn consuming_oneshot() -> Option<OneshotReceiver<u64>> {
    Some(performable_oneshot())
}

#[metabench::benchmark(SINGLE_ONESHOT_RECV, READY_BLOCKING_SINGLE, "oneshot_recv")]
#[bench::value(&mut consuming_oneshot())]
fn single_oneshot_recv(receiver: &mut Option<OneshotReceiver<u64>>) -> u64 {
    black_box(receiver.take().expect("each batch prepares one receiver"))
        .recv()
        .expect(RECEIVE_READY)
}

#[metabench::benchmark(SINGLE_ONESHOT_TIMEOUT, READY_BLOCKING_SINGLE, "oneshot_recv_timeout")]
#[bench::value(&mut consuming_oneshot())]
fn single_oneshot_timeout(receiver: &mut Option<OneshotReceiver<u64>>) -> u64 {
    black_box(receiver.take().expect("each batch prepares one receiver"))
        .recv_timeout(Duration::ZERO)
        .expect(RECEIVE_READY)
}

fn unobserved_watch() -> (WatchSender<u64>, WatchReceiver<u64>) {
    let pair = channel::watch(0);
    pair.0.send(7).expect(SEND_READY);
    pair
}

#[metabench::benchmark(SINGLE_WATCH_CHANGE, READY_BLOCKING_SINGLE, "watch_changed")]
#[bench::value(&mut unobserved_watch())]
fn single_watch_change(pair: &mut (WatchSender<u64>, WatchReceiver<u64>)) {
    black_box(&pair.1).changed().expect("setup published an unobserved version");
}

fn ready_blocking_single_operations(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(READY_BLOCKING_SINGLE);
    for (name, bounded) in [("bounded", true), ("unbounded", false)] {
        group.bench_function(format!("queue_send/{name}"), |bencher| {
            bencher.iter_batched_ref(|| empty_blocking_queue(bounded), single_queue_send, READY_BATCH);
        });
        group.bench_function(format!("queue_recv/{name}"), |bencher| {
            bencher.iter_batched_ref(|| ready_blocking_queue(bounded), single_queue_recv, READY_BATCH);
        });
        group.bench_function(format!("queue_recv_timeout/{name}"), |bencher| {
            bencher.iter_batched_ref(|| ready_blocking_queue(bounded), single_queue_timeout, READY_BATCH);
        });
    }
    group.bench_function(SINGLE_ONESHOT_RECV.benchmark_name(), |bencher| {
        bencher.iter_batched_ref(consuming_oneshot, single_oneshot_recv, READY_BATCH);
    });
    group.bench_function(SINGLE_ONESHOT_TIMEOUT.benchmark_name(), |bencher| {
        bencher.iter_batched_ref(consuming_oneshot, single_oneshot_timeout, READY_BATCH);
    });
    group.bench_function(SINGLE_WATCH_CHANGE.benchmark_name(), |bencher| {
        bencher.iter_batched_ref(unobserved_watch, single_watch_change, READY_BATCH);
    });
    group.finish();
}

struct StandardSignal {
    permit: std::sync::Mutex<bool>,
    changed: std::sync::Condvar,
}

impl StandardSignal {
    const fn new() -> Self {
        Self {
            permit: std::sync::Mutex::new(false),
            changed: std::sync::Condvar::new(),
        }
    }

    fn notify(&self) {
        *self.permit.lock().expect("benchmark callers never panic while holding the state") = true;
        self.changed.notify_one();
    }

    fn wait(&self) {
        let mut permit = self.permit.lock().expect("benchmark callers never panic while holding the state");
        while !*permit {
            permit = self.changed.wait(permit).expect("benchmark callers never poison the state");
        }
        *permit = false;
    }
}

struct PreviousSignal {
    permit: performables::sync::mutex::Mutex<bool>,
    changed: Condvar,
}

impl PreviousSignal {
    fn new() -> Self {
        Self {
            permit: performables::sync::mutex::Mutex::new(false),
            changed: Condvar::new(),
        }
    }

    fn notify(&self) {
        *self.permit.lock() = true;
        self.changed.notify_one();
    }

    fn wait(&self) {
        let mut permit = self.permit.lock();
        while !*permit {
            permit = self.changed.wait(permit);
        }
        *permit = false;
    }
}

struct AsyncNotification {
    notify: Notify<Async>,
    context: Context<'static>,
}

fn async_notification() -> AsyncNotification {
    AsyncNotification {
        notify: Notify::new(),
        context: Context::from_waker(Waker::noop()),
    }
}

struct TokioNotification {
    notify: tokio::sync::Notify,
    context: Context<'static>,
}

fn tokio_notification() -> TokioNotification {
    TokioNotification {
        notify: tokio::sync::Notify::new(),
        context: Context::from_waker(Waker::noop()),
    }
}

#[metabench::benchmark(PERMIT_SYNC, PERMIT_COALESCED, "performables_sync")]
#[bench::value(&{ let notify: Notify = Notify::new(); notify.notify_one(); notify })]
fn permit_coalesced_sync(notify: &Notify) {
    black_box(notify).notify_one();
}

#[metabench::benchmark(PERMIT_ASYNC, PERMIT_COALESCED, "performables_async")]
#[bench::value(&{ let notify = Notify::<Async>::new(); notify.notify_one(); notify })]
fn permit_coalesced_async(notify: &Notify<Async>) {
    black_box(notify).notify_one();
}

#[metabench::benchmark(PERMIT_STD, PERMIT_COALESCED, "std")]
#[bench::value(&{ let signal = StandardSignal::new(); signal.notify(); signal })]
fn permit_coalesced_std(signal: &StandardSignal) {
    black_box(signal).notify();
}

#[metabench::benchmark(PERMIT_PREVIOUS, PERMIT_COALESCED, "previous_signal")]
#[bench::value(&{ let signal = PreviousSignal::new(); signal.notify(); signal })]
fn permit_coalesced_previous(signal: &PreviousSignal) {
    black_box(signal).notify();
}

#[metabench::benchmark(PERMIT_TOKIO, PERMIT_COALESCED, "tokio")]
#[bench::value(&{ let notify = tokio::sync::Notify::new(); notify.notify_one(); notify })]
fn permit_coalesced_tokio(notify: &tokio::sync::Notify) {
    black_box(notify).notify_one();
}

#[metabench::benchmark(PERMIT_CYCLE_SYNC, PERMIT_CYCLE, "performables_sync")]
#[bench::value(&Notify::new())]
fn permit_cycle_sync(notify: &Notify) {
    let notify = black_box(notify);
    notify.notify_one();
    notify.wait();
}

#[metabench::benchmark(PERMIT_CYCLE_ASYNC, PERMIT_CYCLE, "performables_async")]
#[bench::value(&mut async_notification())]
fn permit_cycle_async(input: &mut AsyncNotification) {
    let input = black_box(input);
    input.notify.notify_one();
    assert!(pin!(input.notify.wait_async()).as_mut().poll(&mut input.context).is_ready());
}

#[metabench::benchmark(PERMIT_CYCLE_TOKIO, PERMIT_CYCLE, "tokio")]
#[bench::value(&mut tokio_notification())]
fn permit_cycle_tokio(input: &mut TokioNotification) {
    let input = black_box(input);
    input.notify.notify_one();
    assert!(pin!(input.notify.notified()).as_mut().poll(&mut input.context).is_ready());
}

#[metabench::benchmark(PERMIT_CYCLE_STD, PERMIT_CYCLE, "std")]
#[bench::value(&StandardSignal::new())]
fn permit_cycle_std(signal: &StandardSignal) {
    let signal = black_box(signal);
    signal.notify();
    signal.wait();
}

#[metabench::benchmark(PERMIT_CYCLE_PREVIOUS, PERMIT_CYCLE, "previous_signal")]
#[bench::value(&PreviousSignal::new())]
fn permit_cycle_previous(signal: &PreviousSignal) {
    let signal = black_box(signal);
    signal.notify();
    signal.wait();
}

fn ready<F: Future>(future: F) -> F::Output {
    let mut context = Context::from_waker(Waker::noop());
    // Stack-pin to avoid allocator noise on the measured path.
    let mut future = pin!(future);
    let Poll::Ready(output) = future.as_mut().poll(&mut context) else {
        unreachable!("the prepared benchmark input must be ready");
    };
    output
}

fn standard_queue() -> (mpsc::SyncSender<u64>, mpsc::Receiver<u64>) {
    let input = mpsc::sync_channel(QUEUE_CAPACITY);
    input.0.send(7).expect(SEND_READY);
    assert_eq!(input.1.recv().expect(RECEIVE_READY), 7);
    input
}

fn performable_queue() -> (Sender<u64>, Receiver<u64>) {
    let input = channel::bounded(QUEUE_CAPACITY);
    input.0.try_send(7).expect(SEND_READY);
    assert_eq!(input.1.try_recv().expect(RECEIVE_READY), 7);
    input
}

fn tokio_queue() -> (tokio::sync::mpsc::Sender<u64>, tokio::sync::mpsc::Receiver<u64>) {
    let mut input = tokio::sync::mpsc::channel(QUEUE_CAPACITY);
    input.0.try_send(7).expect(SEND_READY);
    assert_eq!(input.1.try_recv().expect(RECEIVE_READY), 7);
    input
}

fn tokio_blocking_queue() -> (tokio::sync::mpsc::Sender<u64>, tokio::sync::mpsc::Receiver<u64>) {
    let mut input = tokio_queue();
    // Tokio's blocking bridge initializes thread-local parking state once.
    input.0.blocking_send(7).expect(SEND_READY);
    assert_eq!(input.1.blocking_recv().expect(RECEIVE_READY), 7);
    input
}

fn standard_unbounded() -> (mpsc::Sender<u64>, mpsc::Receiver<u64>) {
    let input = mpsc::channel();
    input.0.send(7).expect(SEND_READY);
    assert_eq!(input.1.recv().expect(RECEIVE_READY), 7);
    input
}

fn performable_unbounded() -> (Sender<u64>, Receiver<u64>) {
    let input = channel::unbounded();
    input.0.try_send(7).expect(SEND_READY);
    assert_eq!(input.1.try_recv().expect(RECEIVE_READY), 7);
    input
}

fn tokio_unbounded() -> (tokio::sync::mpsc::UnboundedSender<u64>, tokio::sync::mpsc::UnboundedReceiver<u64>) {
    let mut input = tokio::sync::mpsc::unbounded_channel();
    input.0.send(7).expect(SEND_READY);
    assert_eq!(input.1.try_recv().expect(RECEIVE_READY), 7);
    input
}

fn standard_lazy() -> std::sync::LazyLock<u64> {
    let value: std::sync::LazyLock<u64> = std::sync::LazyLock::new(|| 7);
    assert_eq!(*std::sync::LazyLock::force(&value), 7);
    value
}

fn performable_lazy() -> LazyLock<u64> {
    let value: LazyLock<u64> = LazyLock::new(|| 7);
    assert_eq!(*LazyLock::force(&value), 7);
    value
}

fn performable_oneshot() -> OneshotReceiver<u64> {
    let (sender, receiver) = channel::oneshot();
    sender.send(7).expect("the receiver remains alive during setup");
    receiver
}

fn tokio_oneshot() -> tokio::sync::oneshot::Receiver<u64> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    sender.send(7).expect("the receiver remains alive during setup");
    receiver
}

#[metabench::benchmark(ARC_STD, ARC_CYCLE, "std")]
#[bench::value(&std::sync::Arc::new(7_u64))]
fn arc_std(value: &std::sync::Arc<u64>) {
    drop(black_box(std::sync::Arc::clone(black_box(value))));
}

#[metabench::benchmark(ARC_PERFORMABLES, ARC_CYCLE, "performables")]
#[bench::value(&Arc::new(7_u64))]
fn arc_performables(value: &Arc<u64>) {
    drop(black_box(Arc::clone(black_box(value))));
}

#[metabench::benchmark(BARRIER_STD, BARRIER, "std")]
#[bench::value(&std::sync::Barrier::new(1))]
fn barrier_std(value: &std::sync::Barrier) -> bool {
    black_box(black_box(value).wait().is_leader())
}

#[metabench::benchmark(BARRIER_SYNC, BARRIER, "performables_sync")]
#[bench::value(&Barrier::new(1))]
fn barrier_sync(value: &Barrier) -> bool {
    black_box(black_box(value).wait().is_leader())
}

#[metabench::benchmark(BARRIER_ASYNC, BARRIER, "performables_async")]
#[bench::value(&Barrier::<Async>::new(1))]
fn barrier_async(value: &Barrier<Async>) -> bool {
    black_box(ready(black_box(value).wait_async()).is_leader())
}

#[metabench::benchmark(BARRIER_BLOCKING, BARRIER, "performables_async_blocking")]
#[bench::value(&Barrier::<Async>::new(1))]
fn barrier_blocking(value: &Barrier<Async>) -> bool {
    black_box(black_box(value).wait().is_leader())
}

#[metabench::benchmark(BARRIER_TOKIO, BARRIER, "tokio")]
#[bench::value(&tokio::sync::Barrier::new(1))]
fn barrier_tokio(value: &tokio::sync::Barrier) -> bool {
    black_box(ready(black_box(value).wait()).is_leader())
}

#[metabench::benchmark(ONCE_STD, ONCE, "std")]
#[bench::value(&std::sync::OnceLock::from(7_u64))]
fn once_std(value: &std::sync::OnceLock<u64>) -> u64 {
    black_box(*black_box(value).get_or_init(|| unreachable!("the cell is initialized")))
}

#[metabench::benchmark(ONCE_PERFORMABLES, ONCE, "performables")]
#[bench::value(&OnceLock::from(7_u64))]
fn once_performables(value: &OnceLock<u64>) -> u64 {
    black_box(*black_box(value).get_or_init(|| unreachable!("the cell is initialized")))
}

#[metabench::benchmark(ONCE_TOKIO, ONCE, "tokio")]
#[bench::value(&tokio::sync::OnceCell::new_with(Some(7_u64)))]
fn once_tokio(value: &tokio::sync::OnceCell<u64>) -> u64 {
    black_box(*ready(
        black_box(value).get_or_init(|| async { unreachable!("the cell is initialized") }),
    ))
}

#[metabench::benchmark(LAZY_STD, LAZY, "std")]
#[bench::value(&standard_lazy())]
fn lazy_std(value: &std::sync::LazyLock<u64>) -> u64 {
    black_box(**black_box(value))
}

#[metabench::benchmark(LAZY_PERFORMABLES, LAZY, "performables")]
#[bench::value(&performable_lazy())]
fn lazy_performables(value: &LazyLock<u64>) -> u64 {
    black_box(**black_box(value))
}

#[metabench::benchmark(NOTIFY_STD, NOTIFY, "std")]
#[bench::value(&std::sync::Condvar::new())]
fn notify_std(value: &std::sync::Condvar) {
    black_box(value).notify_one();
}

#[metabench::benchmark(NOTIFY_SYNC, NOTIFY, "performables_sync")]
#[bench::value(&Condvar::new())]
fn notify_sync(value: &Condvar) {
    black_box(value).notify_one();
}

#[metabench::benchmark(NOTIFY_ASYNC, NOTIFY, "performables_async")]
#[bench::value(&Condvar::<Async>::new())]
fn notify_async(value: &Condvar<Async>) {
    black_box(value).notify_one();
}

#[metabench::benchmark(QUEUE_TRY_STD, QUEUE_TRY, "std")]
#[bench::value(&mut standard_queue())]
fn queue_try_std(input: &mut (mpsc::SyncSender<u64>, mpsc::Receiver<u64>)) -> u64 {
    let input = black_box(input);
    input.0.try_send(black_box(7)).expect(SEND_READY);
    black_box(input.1.try_recv().expect(RECEIVE_READY))
}

#[metabench::benchmark(QUEUE_TRY_PERFORMABLES, QUEUE_TRY, "performables")]
#[bench::value(&mut performable_queue())]
fn queue_try_performables(input: &mut (Sender<u64>, Receiver<u64>)) -> u64 {
    let input = black_box(input);
    input.0.try_send(black_box(7)).expect(SEND_READY);
    black_box(input.1.try_recv().expect(RECEIVE_READY))
}

#[metabench::benchmark(QUEUE_TRY_TOKIO, QUEUE_TRY, "tokio")]
#[bench::value(&mut tokio_queue())]
fn queue_try_tokio(input: &mut (tokio::sync::mpsc::Sender<u64>, tokio::sync::mpsc::Receiver<u64>)) -> u64 {
    let input = black_box(input);
    input.0.try_send(black_box(7)).expect(SEND_READY);
    black_box(input.1.try_recv().expect(RECEIVE_READY))
}

#[metabench::benchmark(QUEUE_BLOCKING_STD, QUEUE_BLOCKING, "std")]
#[bench::value(&mut standard_queue())]
fn queue_blocking_std(input: &mut (mpsc::SyncSender<u64>, mpsc::Receiver<u64>)) -> u64 {
    let input = black_box(input);
    input.0.send(black_box(7)).expect(SEND_READY);
    black_box(input.1.recv().expect(RECEIVE_READY))
}

#[metabench::benchmark(QUEUE_BLOCKING_PERFORMABLES, QUEUE_BLOCKING, "performables")]
#[bench::value(&mut performable_queue())]
fn queue_blocking_performables(input: &mut (Sender<u64>, Receiver<u64>)) -> u64 {
    let input = black_box(input);
    input.0.send(black_box(7)).expect(SEND_READY);
    black_box(input.1.recv().expect(RECEIVE_READY))
}

#[metabench::benchmark(QUEUE_BLOCKING_TOKIO, QUEUE_BLOCKING, "tokio")]
#[bench::value(&mut tokio_blocking_queue())]
fn queue_blocking_tokio(input: &mut (tokio::sync::mpsc::Sender<u64>, tokio::sync::mpsc::Receiver<u64>)) -> u64 {
    let input = black_box(input);
    input.0.blocking_send(black_box(7)).expect(SEND_READY);
    black_box(input.1.blocking_recv().expect(RECEIVE_READY))
}

#[metabench::benchmark(QUEUE_ASYNC_PERFORMABLES, QUEUE_ASYNC, "performables")]
#[bench::value(&mut performable_queue())]
fn queue_async_performables(input: &mut (Sender<u64>, Receiver<u64>)) -> u64 {
    let input = black_box(input);
    ready(input.0.send_async(black_box(7))).expect(SEND_READY);
    black_box(ready(input.1.recv_async()).expect(RECEIVE_READY))
}

#[metabench::benchmark(QUEUE_ASYNC_TOKIO, QUEUE_ASYNC, "tokio")]
#[bench::value(&mut tokio_queue())]
fn queue_async_tokio(input: &mut (tokio::sync::mpsc::Sender<u64>, tokio::sync::mpsc::Receiver<u64>)) -> u64 {
    let input = black_box(input);
    ready(input.0.send(black_box(7))).expect(SEND_READY);
    black_box(ready(input.1.recv()).expect(RECEIVE_READY))
}

#[metabench::benchmark(UNBOUNDED_STD, UNBOUNDED, "std")]
#[bench::value(&mut standard_unbounded())]
fn unbounded_std(input: &mut (mpsc::Sender<u64>, mpsc::Receiver<u64>)) -> u64 {
    let input = black_box(input);
    input.0.send(black_box(7)).expect(SEND_READY);
    black_box(input.1.try_recv().expect(RECEIVE_READY))
}

#[metabench::benchmark(UNBOUNDED_PERFORMABLES, UNBOUNDED, "performables")]
#[bench::value(&mut performable_unbounded())]
fn unbounded_performables(input: &mut (Sender<u64>, Receiver<u64>)) -> u64 {
    let input = black_box(input);
    input.0.try_send(black_box(7)).expect(SEND_READY);
    black_box(input.1.try_recv().expect(RECEIVE_READY))
}

#[metabench::benchmark(UNBOUNDED_TOKIO, UNBOUNDED, "tokio")]
#[bench::value(&mut tokio_unbounded())]
fn unbounded_tokio(input: &mut (tokio::sync::mpsc::UnboundedSender<u64>, tokio::sync::mpsc::UnboundedReceiver<u64>)) -> u64 {
    let input = black_box(input);
    input.0.send(black_box(7)).expect(SEND_READY);
    black_box(input.1.try_recv().expect(RECEIVE_READY))
}

#[metabench::benchmark(WATCH_READ_PERFORMABLES, WATCH_READ, "performables")]
#[bench::value(&channel::watch(7_u64))]
fn watch_read_performables(input: &(WatchSender<u64>, WatchReceiver<u64>)) -> u64 {
    black_box(*black_box(input).1.borrow())
}

#[metabench::benchmark(WATCH_READ_TOKIO, WATCH_READ, "tokio")]
#[bench::value(&tokio::sync::watch::channel(7_u64))]
fn watch_read_tokio(input: &(tokio::sync::watch::Sender<u64>, tokio::sync::watch::Receiver<u64>)) -> u64 {
    black_box(*black_box(input).1.borrow())
}

#[metabench::benchmark(WATCH_WRITE_PERFORMABLES, WATCH_WRITE, "performables")]
#[bench::value(&channel::watch(7_u64))]
fn watch_write_performables(input: &(WatchSender<u64>, WatchReceiver<u64>)) -> u64 {
    black_box(black_box(input).0.send_replace(black_box(7)))
}

#[metabench::benchmark(WATCH_WRITE_TOKIO, WATCH_WRITE, "tokio")]
#[bench::value(&tokio::sync::watch::channel(7_u64))]
fn watch_write_tokio(input: &(tokio::sync::watch::Sender<u64>, tokio::sync::watch::Receiver<u64>)) -> u64 {
    black_box(black_box(input).0.send_replace(black_box(7)))
}

#[metabench::benchmark(ONESHOT_PERFORMABLES, ONESHOT, "performables")]
#[bench::value(&mut performable_oneshot())]
fn oneshot_performables(value: &mut OneshotReceiver<u64>) -> u64 {
    black_box(
        black_box(value)
            .try_recv()
            .expect("setup delivered a value and retained this receiver"),
    )
}

#[metabench::benchmark(ONESHOT_TOKIO, ONESHOT, "tokio")]
#[bench::value(&mut tokio_oneshot())]
fn oneshot_tokio(value: &mut tokio::sync::oneshot::Receiver<u64>) -> u64 {
    black_box(
        black_box(value)
            .try_recv()
            .expect("setup delivered a value and retained this receiver"),
    )
}

fn ownership_and_initialization(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(ARC_CYCLE);
    let standard = std::sync::Arc::new(7_u64);
    let performable = Arc::new(7_u64);
    group.bench_function(ARC_STD.benchmark_name(), |b| b.iter(|| arc_std(&standard)));
    group.bench_function(ARC_PERFORMABLES.benchmark_name(), |b| b.iter(|| arc_performables(&performable)));
    group.finish();

    let mut group = criterion.benchmark_group(ONCE);
    let standard = std::sync::OnceLock::from(7_u64);
    let performable = OnceLock::from(7_u64);
    let tokio = tokio::sync::OnceCell::new_with(Some(7_u64));
    group.bench_function(ONCE_STD.benchmark_name(), |b| b.iter(|| once_std(&standard)));
    group.bench_function(ONCE_PERFORMABLES.benchmark_name(), |b| b.iter(|| once_performables(&performable)));
    group.bench_function(ONCE_TOKIO.benchmark_name(), |b| b.iter(|| once_tokio(&tokio)));
    group.finish();

    let mut group = criterion.benchmark_group(LAZY);
    let standard = standard_lazy();
    let performable = performable_lazy();
    group.bench_function(LAZY_STD.benchmark_name(), |b| b.iter(|| lazy_std(&standard)));
    group.bench_function(LAZY_PERFORMABLES.benchmark_name(), |b| b.iter(|| lazy_performables(&performable)));
    group.finish();
}

fn barriers_and_notifications(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(BARRIER);
    let standard = std::sync::Barrier::new(1);
    let synchronous = Barrier::new(1);
    let asynchronous = Barrier::<Async>::new(1);
    let tokio = tokio::sync::Barrier::new(1);
    group.bench_function(BARRIER_STD.benchmark_name(), |b| b.iter(|| barrier_std(&standard)));
    group.bench_function(BARRIER_SYNC.benchmark_name(), |b| b.iter(|| barrier_sync(&synchronous)));
    group.bench_function(BARRIER_ASYNC.benchmark_name(), |b| b.iter(|| barrier_async(&asynchronous)));
    group.bench_function(BARRIER_BLOCKING.benchmark_name(), |b| b.iter(|| barrier_blocking(&asynchronous)));
    group.bench_function(BARRIER_TOKIO.benchmark_name(), |b| b.iter(|| barrier_tokio(&tokio)));
    group.finish();

    let mut group = criterion.benchmark_group(NOTIFY);
    let standard = std::sync::Condvar::new();
    let synchronous = Condvar::new();
    let asynchronous = Condvar::<Async>::new();
    group.bench_function(NOTIFY_STD.benchmark_name(), |b| b.iter(|| notify_std(&standard)));
    group.bench_function(NOTIFY_SYNC.benchmark_name(), |b| b.iter(|| notify_sync(&synchronous)));
    group.bench_function(NOTIFY_ASYNC.benchmark_name(), |b| b.iter(|| notify_async(&asynchronous)));
    group.finish();
}

fn queues(criterion: &mut Criterion) {
    let mut standard = standard_queue();
    let mut performable = performable_queue();
    let mut tokio = tokio_queue();
    let mut group = criterion.benchmark_group(QUEUE_TRY);
    group.bench_function(QUEUE_TRY_STD.benchmark_name(), |b| b.iter(|| queue_try_std(&mut standard)));
    group.bench_function(QUEUE_TRY_PERFORMABLES.benchmark_name(), |b| {
        b.iter(|| queue_try_performables(&mut performable));
    });
    group.bench_function(QUEUE_TRY_TOKIO.benchmark_name(), |b| b.iter(|| queue_try_tokio(&mut tokio)));
    group.finish();

    let mut group = criterion.benchmark_group(QUEUE_BLOCKING);
    let mut tokio_blocking = tokio_blocking_queue();
    group.bench_function(QUEUE_BLOCKING_STD.benchmark_name(), |b| {
        b.iter(|| queue_blocking_std(&mut standard));
    });
    group.bench_function(QUEUE_BLOCKING_PERFORMABLES.benchmark_name(), |b| {
        b.iter(|| queue_blocking_performables(&mut performable));
    });
    group.bench_function(QUEUE_BLOCKING_TOKIO.benchmark_name(), |b| {
        b.iter(|| queue_blocking_tokio(&mut tokio_blocking));
    });
    group.finish();

    let mut group = criterion.benchmark_group(QUEUE_ASYNC);
    group.bench_function(QUEUE_ASYNC_PERFORMABLES.benchmark_name(), |b| {
        b.iter(|| queue_async_performables(&mut performable));
    });
    group.bench_function(QUEUE_ASYNC_TOKIO.benchmark_name(), |b| b.iter(|| queue_async_tokio(&mut tokio)));
    group.finish();

    let mut group = criterion.benchmark_group(UNBOUNDED);
    let mut standard = standard_unbounded();
    let mut performable = performable_unbounded();
    let mut tokio = tokio_unbounded();
    group.bench_function(UNBOUNDED_STD.benchmark_name(), |b| b.iter(|| unbounded_std(&mut standard)));
    group.bench_function(UNBOUNDED_PERFORMABLES.benchmark_name(), |b| {
        b.iter(|| unbounded_performables(&mut performable));
    });
    group.bench_function(UNBOUNDED_TOKIO.benchmark_name(), |b| b.iter(|| unbounded_tokio(&mut tokio)));
    group.finish();
}

fn watch_and_oneshot(criterion: &mut Criterion) {
    let performable = channel::watch(7_u64);
    let tokio = tokio::sync::watch::channel(7_u64);
    let mut group = criterion.benchmark_group(WATCH_READ);
    group.bench_function(WATCH_READ_PERFORMABLES.benchmark_name(), |b| {
        b.iter(|| watch_read_performables(&performable));
    });
    group.bench_function(WATCH_READ_TOKIO.benchmark_name(), |b| b.iter(|| watch_read_tokio(&tokio)));
    group.finish();

    let mut group = criterion.benchmark_group(WATCH_WRITE);
    group.bench_function(WATCH_WRITE_PERFORMABLES.benchmark_name(), |b| {
        b.iter(|| watch_write_performables(&performable));
    });
    group.bench_function(WATCH_WRITE_TOKIO.benchmark_name(), |b| b.iter(|| watch_write_tokio(&tokio)));
    group.finish();

    let mut group = criterion.benchmark_group(ONESHOT);
    group.bench_function(ONESHOT_PERFORMABLES.benchmark_name(), |b| {
        b.iter_batched_ref(performable_oneshot, oneshot_performables, BatchSize::SmallInput);
    });
    group.bench_function(ONESHOT_TOKIO.benchmark_name(), |b| {
        b.iter_batched_ref(tokio_oneshot, oneshot_tokio, BatchSize::SmallInput);
    });
    group.finish();
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    #[cfg(feature = "seismograph")]
    seismograph::recorder(black_box(seismograph::recorder::Configuration::default()));

    ownership_and_initialization(criterion);
    barriers_and_notifications(criterion);
    queues(criterion);
    watch_and_oneshot(criterion);
    notification_permits(criterion);
    ready_blocking_single_operations(criterion);
}

fn notification_permits(criterion: &mut Criterion) {
    let sync: Notify = Notify::new();
    let asynchronous = Notify::<Async>::new();
    let std = StandardSignal::new();
    let previous = PreviousSignal::new();
    let tokio = tokio::sync::Notify::new();
    sync.notify_one();
    asynchronous.notify_one();
    std.notify();
    previous.notify();
    tokio.notify_one();
    let mut group = criterion.benchmark_group(PERMIT_COALESCED);
    group.bench_function(PERMIT_SYNC.benchmark_name(), |b| b.iter(|| permit_coalesced_sync(&sync)));
    group.bench_function(PERMIT_ASYNC.benchmark_name(), |b| b.iter(|| permit_coalesced_async(&asynchronous)));
    group.bench_function(PERMIT_STD.benchmark_name(), |b| b.iter(|| permit_coalesced_std(&std)));
    group.bench_function(PERMIT_PREVIOUS.benchmark_name(), |b| {
        b.iter(|| permit_coalesced_previous(&previous));
    });
    group.bench_function(PERMIT_TOKIO.benchmark_name(), |b| b.iter(|| permit_coalesced_tokio(&tokio)));
    group.finish();
    let mut asynchronous = async_notification();
    let mut tokio = tokio_notification();
    let mut group = criterion.benchmark_group(PERMIT_CYCLE);
    group.bench_function(PERMIT_CYCLE_SYNC.benchmark_name(), |b| b.iter(|| permit_cycle_sync(&sync)));
    group.bench_function(PERMIT_CYCLE_ASYNC.benchmark_name(), |b| {
        b.iter(|| permit_cycle_async(&mut asynchronous));
    });
    group.bench_function(PERMIT_CYCLE_STD.benchmark_name(), |b| b.iter(|| permit_cycle_std(&std)));
    group.bench_function(PERMIT_CYCLE_PREVIOUS.benchmark_name(), |b| {
        b.iter(|| permit_cycle_previous(&previous));
    });
    group.bench_function(PERMIT_CYCLE_TOKIO.benchmark_name(), |b| b.iter(|| permit_cycle_tokio(&mut tokio)));
    group.finish();
}

metabench::main!(
    criterion = criterion_benchmarks,
    benchmarks = [
        ARC_STD,
        ARC_PERFORMABLES,
        BARRIER_STD,
        BARRIER_SYNC,
        BARRIER_ASYNC,
        BARRIER_BLOCKING,
        BARRIER_TOKIO,
        ONCE_STD,
        ONCE_PERFORMABLES,
        ONCE_TOKIO,
        LAZY_STD,
        LAZY_PERFORMABLES,
        NOTIFY_STD,
        NOTIFY_SYNC,
        NOTIFY_ASYNC,
        QUEUE_TRY_STD,
        QUEUE_TRY_PERFORMABLES,
        QUEUE_TRY_TOKIO,
        QUEUE_BLOCKING_STD,
        QUEUE_BLOCKING_PERFORMABLES,
        QUEUE_BLOCKING_TOKIO,
        QUEUE_ASYNC_PERFORMABLES,
        QUEUE_ASYNC_TOKIO,
        UNBOUNDED_STD,
        UNBOUNDED_PERFORMABLES,
        UNBOUNDED_TOKIO,
        WATCH_READ_PERFORMABLES,
        WATCH_READ_TOKIO,
        WATCH_WRITE_PERFORMABLES,
        WATCH_WRITE_TOKIO,
        ONESHOT_PERFORMABLES,
        ONESHOT_TOKIO,
        PERMIT_SYNC,
        PERMIT_ASYNC,
        PERMIT_STD,
        PERMIT_PREVIOUS,
        PERMIT_TOKIO,
        PERMIT_CYCLE_SYNC,
        PERMIT_CYCLE_ASYNC,
        PERMIT_CYCLE_STD,
        PERMIT_CYCLE_PREVIOUS,
        PERMIT_CYCLE_TOKIO,
        SINGLE_QUEUE_SEND,
        SINGLE_QUEUE_RECV,
        SINGLE_QUEUE_TIMEOUT,
        SINGLE_ONESHOT_RECV,
        SINGLE_ONESHOT_TIMEOUT,
        SINGLE_WATCH_CHANGE,
    ],
);
