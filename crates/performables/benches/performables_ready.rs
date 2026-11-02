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
use performables::sync::barrier::Barrier;
use performables::sync::channel::{self, OneshotReceiver, Receiver, Sender, WatchReceiver, WatchSender};
use performables::sync::condition::Condvar;
use performables::sync::notify::Notify;
use performables::sync::once::{LazyLock, OnceLock};
use performables::sync::{Async, Sync};
#[cfg(feature = "seismograph")]
use seismograph::recorder::Configuration;

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
// Representative spare capacity keeps these single-message operations ready.
const QUEUE_CAPACITY: usize = 64;
const SEND_READY: &str = "the queue has spare capacity and a live receiver";
const RECEIVE_READY: &str = "the preceding send queued one value";
const READY_BLOCKING_SINGLE: &str = "performables_ready/ready_blocking";
// A representative batch amortizes the timing harness while retaining warmed
// queues; this is a measurement choice, not a queue-capacity boundary.
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
#[bench::bounded(Box::new(empty_blocking_queue(true)))]
#[bench::unbounded(Box::new(empty_blocking_queue(false)))]
fn single_queue_send<S: std::borrow::BorrowMut<Queue>>(mut owned_input: S) -> ((), S) {
    {
        let queue = owned_input.borrow_mut();
        black_box(&queue.0).send(black_box(7)).expect(SEND_READY);
    }
    ((), owned_input)
}

#[metabench::benchmark(SINGLE_QUEUE_RECV, READY_BLOCKING_SINGLE, "queue_recv")]
#[bench::bounded(Box::new(ready_blocking_queue(true)))]
#[bench::unbounded(Box::new(ready_blocking_queue(false)))]
fn single_queue_recv<S: std::borrow::BorrowMut<Queue>>(mut owned_input: S) -> (u64, S) {
    let result = {
        let queue = owned_input.borrow_mut();
        black_box(&queue.1).recv().expect(RECEIVE_READY)
    };
    (result, owned_input)
}

#[metabench::benchmark(SINGLE_QUEUE_TIMEOUT, READY_BLOCKING_SINGLE, "queue_recv_timeout")]
#[bench::bounded(Box::new(ready_blocking_queue(true)))]
#[bench::unbounded(Box::new(ready_blocking_queue(false)))]
fn single_queue_timeout<S: std::borrow::BorrowMut<Queue>>(mut owned_input: S) -> (u64, S) {
    let result = {
        let queue = owned_input.borrow_mut();
        black_box(&queue.1).recv_timeout(Duration::ZERO).expect(RECEIVE_READY)
    };
    (result, owned_input)
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "each batched measurement takes one prepared receiver through its Option"
)]
fn consuming_oneshot() -> Option<OneshotReceiver<u64>> {
    Some(performable_oneshot())
}

#[metabench::benchmark(SINGLE_ONESHOT_RECV, READY_BLOCKING_SINGLE, "oneshot_recv")]
#[bench::value(Box::new(consuming_oneshot()))]
fn single_oneshot_recv<S: std::borrow::BorrowMut<Option<OneshotReceiver<u64>>>>(mut owned_input: S) -> (u64, S) {
    let result = {
        let receiver = owned_input.borrow_mut();
        black_box(receiver.take().expect("each batch prepares one receiver"))
            .recv()
            .expect(RECEIVE_READY)
    };
    (result, owned_input)
}

#[metabench::benchmark(SINGLE_ONESHOT_TIMEOUT, READY_BLOCKING_SINGLE, "oneshot_recv_timeout")]
#[bench::value(Box::new(consuming_oneshot()))]
fn single_oneshot_timeout<S: std::borrow::BorrowMut<Option<OneshotReceiver<u64>>>>(mut owned_input: S) -> (u64, S) {
    let result = {
        let receiver = owned_input.borrow_mut();
        black_box(receiver.take().expect("each batch prepares one receiver"))
            .recv_timeout(Duration::ZERO)
            .expect(RECEIVE_READY)
    };
    (result, owned_input)
}

fn unobserved_watch() -> (WatchSender<u64>, WatchReceiver<u64>) {
    let pair = channel::watch(0);
    pair.0.send(7).expect(SEND_READY);
    pair
}

#[metabench::benchmark(SINGLE_WATCH_CHANGE, READY_BLOCKING_SINGLE, "watch_changed")]
#[bench::value(Box::new(unobserved_watch()))]
fn single_watch_change<S: std::borrow::BorrowMut<(WatchSender<u64>, WatchReceiver<u64>)>>(mut owned_input: S) -> ((), S) {
    {
        let pair = owned_input.borrow_mut();
        black_box(&pair.1).changed().expect("setup published an unobserved version");
    }
    ((), owned_input)
}

fn ready_blocking_single_operations(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(READY_BLOCKING_SINGLE);
    for (name, bounded) in [("bounded", true), ("unbounded", false)] {
        group.bench_function(format!("queue_send/{name}"), |bencher| {
            bencher.iter_batched_ref(|| empty_blocking_queue(bounded), |queue| single_queue_send(queue).0, READY_BATCH);
        });
        group.bench_function(format!("queue_recv/{name}"), |bencher| {
            bencher.iter_batched_ref(|| ready_blocking_queue(bounded), |queue| single_queue_recv(queue).0, READY_BATCH);
        });
        group.bench_function(format!("queue_recv_timeout/{name}"), |bencher| {
            bencher.iter_batched_ref(|| ready_blocking_queue(bounded), |queue| single_queue_timeout(queue).0, READY_BATCH);
        });
    }
    group.bench_function(SINGLE_ONESHOT_RECV.benchmark_name(), |bencher| {
        bencher.iter_batched_ref(consuming_oneshot, |receiver| single_oneshot_recv(receiver).0, READY_BATCH);
    });
    group.bench_function(SINGLE_ONESHOT_TIMEOUT.benchmark_name(), |bencher| {
        bencher.iter_batched_ref(consuming_oneshot, |receiver| single_oneshot_timeout(receiver).0, READY_BATCH);
    });
    group.bench_function(SINGLE_WATCH_CHANGE.benchmark_name(), |bencher| {
        bencher.iter_batched_ref(unobserved_watch, |pair| single_watch_change(pair).0, READY_BATCH);
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
#[bench::value(Box::new({ let notify: Notify = Notify::new(); notify.notify_one(); notify }))]
fn permit_coalesced_sync<S: std::borrow::Borrow<Notify>>(owned_input: S) -> ((), S) {
    {
        let notify = owned_input.borrow();
        black_box(notify).notify_one();
    }
    ((), owned_input)
}

#[metabench::benchmark(PERMIT_ASYNC, PERMIT_COALESCED, "performables_async")]
#[bench::value(Box::new({ let notify = Notify::<Async>::new(); notify.notify_one(); notify }))]
fn permit_coalesced_async<S: std::borrow::Borrow<Notify<Async>>>(owned_input: S) -> ((), S) {
    {
        let notify = owned_input.borrow();
        black_box(notify).notify_one();
    }
    ((), owned_input)
}

#[metabench::benchmark(PERMIT_STD, PERMIT_COALESCED, "std")]
#[bench::value(Box::new({ let signal = StandardSignal::new(); signal.notify(); signal }))]
fn permit_coalesced_std<S: std::borrow::Borrow<StandardSignal>>(owned_input: S) -> ((), S) {
    {
        let signal = owned_input.borrow();
        black_box(signal).notify();
    }
    ((), owned_input)
}

#[metabench::benchmark(PERMIT_PREVIOUS, PERMIT_COALESCED, "previous_signal")]
#[bench::value(Box::new({ let signal = PreviousSignal::new(); signal.notify(); signal }))]
fn permit_coalesced_previous<S: std::borrow::Borrow<PreviousSignal>>(owned_input: S) -> ((), S) {
    {
        let signal = owned_input.borrow();
        black_box(signal).notify();
    }
    ((), owned_input)
}

#[metabench::benchmark(PERMIT_TOKIO, PERMIT_COALESCED, "tokio")]
#[bench::value(Box::new({ let notify = tokio::sync::Notify::new(); notify.notify_one(); notify }))]
fn permit_coalesced_tokio<S: std::borrow::Borrow<tokio::sync::Notify>>(owned_input: S) -> ((), S) {
    {
        let notify = owned_input.borrow();
        black_box(notify).notify_one();
    }
    ((), owned_input)
}

#[metabench::benchmark(PERMIT_CYCLE_SYNC, PERMIT_CYCLE, "performables_sync")]
#[bench::value(Box::new(Notify::<Sync>::new()))]
fn permit_cycle_sync<S: std::borrow::Borrow<Notify>>(owned_input: S) -> ((), S) {
    {
        let notify = owned_input.borrow();
        let notify = black_box(notify);
        notify.notify_one();
        notify.wait();
    }
    ((), owned_input)
}

#[metabench::benchmark(PERMIT_CYCLE_ASYNC, PERMIT_CYCLE, "performables_async")]
#[bench::value(Box::new(async_notification()))]
fn permit_cycle_async<S: std::borrow::BorrowMut<AsyncNotification>>(mut owned_input: S) -> ((), S) {
    {
        let input = owned_input.borrow_mut();
        let input = black_box(input);
        input.notify.notify_one();
        assert!(pin!(input.notify.wait_async()).as_mut().poll(&mut input.context).is_ready());
    }
    ((), owned_input)
}

#[metabench::benchmark(PERMIT_CYCLE_TOKIO, PERMIT_CYCLE, "tokio")]
#[bench::value(Box::new(tokio_notification()))]
fn permit_cycle_tokio<S: std::borrow::BorrowMut<TokioNotification>>(mut owned_input: S) -> ((), S) {
    {
        let input = owned_input.borrow_mut();
        let input = black_box(input);
        input.notify.notify_one();
        assert!(pin!(input.notify.notified()).as_mut().poll(&mut input.context).is_ready());
    }
    ((), owned_input)
}

#[metabench::benchmark(PERMIT_CYCLE_STD, PERMIT_CYCLE, "std")]
#[bench::value(Box::new(StandardSignal::new()))]
fn permit_cycle_std<S: std::borrow::Borrow<StandardSignal>>(owned_input: S) -> ((), S) {
    {
        let signal = owned_input.borrow();
        let signal = black_box(signal);
        signal.notify();
        signal.wait();
    }
    ((), owned_input)
}

#[metabench::benchmark(PERMIT_CYCLE_PREVIOUS, PERMIT_CYCLE, "previous_signal")]
#[bench::value(Box::new(PreviousSignal::new()))]
fn permit_cycle_previous<S: std::borrow::Borrow<PreviousSignal>>(owned_input: S) -> ((), S) {
    {
        let signal = owned_input.borrow();
        let signal = black_box(signal);
        signal.notify();
        signal.wait();
    }
    ((), owned_input)
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
#[bench::value(Box::new(std::sync::Arc::new(7_u64)))]
fn arc_std<S: std::borrow::Borrow<std::sync::Arc<u64>>>(owned_input: S) -> ((), S) {
    {
        let value = owned_input.borrow();
        drop(black_box(std::sync::Arc::clone(black_box(value))));
    }
    ((), owned_input)
}

#[metabench::benchmark(ARC_PERFORMABLES, ARC_CYCLE, "performables")]
#[bench::value(Box::new(Arc::new(7_u64)))]
fn arc_performables<S: std::borrow::Borrow<Arc<u64>>>(owned_input: S) -> ((), S) {
    {
        let value = owned_input.borrow();
        drop(black_box(Arc::clone(black_box(value))));
    }
    ((), owned_input)
}

#[metabench::benchmark(BARRIER_STD, BARRIER, "std")]
#[bench::value(Box::new(std::sync::Barrier::new(1)))]
fn barrier_std<S: std::borrow::Borrow<std::sync::Barrier>>(owned_input: S) -> (bool, S) {
    let result = {
        let value = owned_input.borrow();
        black_box(black_box(value).wait().is_leader())
    };
    (result, owned_input)
}

#[metabench::benchmark(BARRIER_SYNC, BARRIER, "performables_sync")]
#[bench::value(Box::new(Barrier::<Sync>::new(1)))]
fn barrier_sync<S: std::borrow::Borrow<Barrier>>(owned_input: S) -> (bool, S) {
    let result = {
        let value = owned_input.borrow();
        black_box(black_box(value).wait().is_leader())
    };
    (result, owned_input)
}

#[metabench::benchmark(BARRIER_ASYNC, BARRIER, "performables_async")]
#[bench::value(Box::new(Barrier::<Async>::new(1)))]
fn barrier_async<S: std::borrow::Borrow<Barrier<Async>>>(owned_input: S) -> (bool, S) {
    let result = {
        let value = owned_input.borrow();
        black_box(ready(black_box(value).wait_async()).is_leader())
    };
    (result, owned_input)
}

#[metabench::benchmark(BARRIER_BLOCKING, BARRIER, "performables_async_blocking")]
#[bench::value(Box::new(Barrier::<Async>::new(1)))]
fn barrier_blocking<S: std::borrow::Borrow<Barrier<Async>>>(owned_input: S) -> (bool, S) {
    let result = {
        let value = owned_input.borrow();
        black_box(black_box(value).wait().is_leader())
    };
    (result, owned_input)
}

#[metabench::benchmark(BARRIER_TOKIO, BARRIER, "tokio")]
#[bench::value(Box::new(tokio::sync::Barrier::new(1)))]
fn barrier_tokio<S: std::borrow::Borrow<tokio::sync::Barrier>>(owned_input: S) -> (bool, S) {
    let result = {
        let value = owned_input.borrow();
        black_box(ready(black_box(value).wait()).is_leader())
    };
    (result, owned_input)
}

#[metabench::benchmark(ONCE_STD, ONCE, "std")]
#[bench::value(Box::new(std::sync::OnceLock::from(7_u64)))]
fn once_std<S: std::borrow::Borrow<std::sync::OnceLock<u64>>>(owned_input: S) -> (u64, S) {
    let result = {
        let value = owned_input.borrow();
        black_box(*black_box(value).get_or_init(|| unreachable!("the cell is initialized")))
    };
    (result, owned_input)
}

#[metabench::benchmark(ONCE_PERFORMABLES, ONCE, "performables")]
#[bench::value(Box::new(OnceLock::from(7_u64)))]
fn once_performables<S: std::borrow::Borrow<OnceLock<u64>>>(owned_input: S) -> (u64, S) {
    let result = {
        let value = owned_input.borrow();
        black_box(*black_box(value).get_or_init(|| unreachable!("the cell is initialized")))
    };
    (result, owned_input)
}

#[metabench::benchmark(ONCE_TOKIO, ONCE, "tokio")]
#[bench::value(Box::new(tokio::sync::OnceCell::new_with(Some(7_u64))))]
fn once_tokio<S: std::borrow::Borrow<tokio::sync::OnceCell<u64>>>(owned_input: S) -> (u64, S) {
    let result = {
        let value = owned_input.borrow();
        black_box(*ready(
            black_box(value).get_or_init(|| async { unreachable!("the cell is initialized") }),
        ))
    };
    (result, owned_input)
}

#[metabench::benchmark(LAZY_STD, LAZY, "std")]
#[bench::value(Box::new(standard_lazy()))]
fn lazy_std<S: std::borrow::Borrow<std::sync::LazyLock<u64>>>(owned_input: S) -> (u64, S) {
    let result = {
        let value = owned_input.borrow();
        black_box(**black_box(value))
    };
    (result, owned_input)
}

#[metabench::benchmark(LAZY_PERFORMABLES, LAZY, "performables")]
#[bench::value(Box::new(performable_lazy()))]
fn lazy_performables<S: std::borrow::Borrow<LazyLock<u64>>>(owned_input: S) -> (u64, S) {
    let result = {
        let value = owned_input.borrow();
        black_box(**black_box(value))
    };
    (result, owned_input)
}

#[metabench::benchmark(NOTIFY_STD, NOTIFY, "std")]
#[bench::value(Box::new(std::sync::Condvar::new()))]
fn notify_std<S: std::borrow::Borrow<std::sync::Condvar>>(owned_input: S) -> ((), S) {
    {
        let value = owned_input.borrow();
        black_box(value).notify_one();
    }
    ((), owned_input)
}

#[metabench::benchmark(NOTIFY_SYNC, NOTIFY, "performables_sync")]
#[bench::value(Box::new(Condvar::<Sync>::new()))]
fn notify_sync<S: std::borrow::Borrow<Condvar>>(owned_input: S) -> ((), S) {
    {
        let value = owned_input.borrow();
        black_box(value).notify_one();
    }
    ((), owned_input)
}

#[metabench::benchmark(NOTIFY_ASYNC, NOTIFY, "performables_async")]
#[bench::value(Box::new(Condvar::<Async>::new()))]
fn notify_async<S: std::borrow::Borrow<Condvar<Async>>>(owned_input: S) -> ((), S) {
    {
        let value = owned_input.borrow();
        black_box(value).notify_one();
    }
    ((), owned_input)
}

#[metabench::benchmark(QUEUE_TRY_STD, QUEUE_TRY, "std")]
#[bench::value(Box::new(standard_queue()))]
fn queue_try_std<S: std::borrow::BorrowMut<(mpsc::SyncSender<u64>, mpsc::Receiver<u64>)>>(mut owned_input: S) -> (u64, S) {
    let result = {
        let input = owned_input.borrow_mut();
        let input = black_box(input);
        input.0.try_send(black_box(7)).expect(SEND_READY);
        black_box(input.1.try_recv().expect(RECEIVE_READY))
    };
    (result, owned_input)
}

#[metabench::benchmark(QUEUE_TRY_PERFORMABLES, QUEUE_TRY, "performables")]
#[bench::value(Box::new(performable_queue()))]
fn queue_try_performables<S: std::borrow::BorrowMut<(Sender<u64>, Receiver<u64>)>>(mut owned_input: S) -> (u64, S) {
    let result = {
        let input = owned_input.borrow_mut();
        let input = black_box(input);
        input.0.try_send(black_box(7)).expect(SEND_READY);
        black_box(input.1.try_recv().expect(RECEIVE_READY))
    };
    (result, owned_input)
}

#[metabench::benchmark(QUEUE_TRY_TOKIO, QUEUE_TRY, "tokio")]
#[bench::value(Box::new(tokio_queue()))]
fn queue_try_tokio<S: std::borrow::BorrowMut<(tokio::sync::mpsc::Sender<u64>, tokio::sync::mpsc::Receiver<u64>)>>(
    mut owned_input: S,
) -> (u64, S) {
    let result = {
        let input = owned_input.borrow_mut();
        let input = black_box(input);
        input.0.try_send(black_box(7)).expect(SEND_READY);
        black_box(input.1.try_recv().expect(RECEIVE_READY))
    };
    (result, owned_input)
}

#[metabench::benchmark(QUEUE_BLOCKING_STD, QUEUE_BLOCKING, "std")]
#[bench::value(Box::new(standard_queue()))]
fn queue_blocking_std<S: std::borrow::BorrowMut<(mpsc::SyncSender<u64>, mpsc::Receiver<u64>)>>(mut owned_input: S) -> (u64, S) {
    let result = {
        let input = owned_input.borrow_mut();
        let input = black_box(input);
        input.0.send(black_box(7)).expect(SEND_READY);
        black_box(input.1.recv().expect(RECEIVE_READY))
    };
    (result, owned_input)
}

#[metabench::benchmark(QUEUE_BLOCKING_PERFORMABLES, QUEUE_BLOCKING, "performables")]
#[bench::value(Box::new(performable_queue()))]
fn queue_blocking_performables<S: std::borrow::BorrowMut<(Sender<u64>, Receiver<u64>)>>(mut owned_input: S) -> (u64, S) {
    let result = {
        let input = owned_input.borrow_mut();
        let input = black_box(input);
        input.0.send(black_box(7)).expect(SEND_READY);
        black_box(input.1.recv().expect(RECEIVE_READY))
    };
    (result, owned_input)
}

#[metabench::benchmark(QUEUE_BLOCKING_TOKIO, QUEUE_BLOCKING, "tokio")]
#[bench::value(Box::new(tokio_blocking_queue()))]
fn queue_blocking_tokio<S: std::borrow::BorrowMut<(tokio::sync::mpsc::Sender<u64>, tokio::sync::mpsc::Receiver<u64>)>>(
    mut owned_input: S,
) -> (u64, S) {
    let result = {
        let input = owned_input.borrow_mut();
        let input = black_box(input);
        input.0.blocking_send(black_box(7)).expect(SEND_READY);
        black_box(input.1.blocking_recv().expect(RECEIVE_READY))
    };
    (result, owned_input)
}

#[metabench::benchmark(QUEUE_ASYNC_PERFORMABLES, QUEUE_ASYNC, "performables")]
#[bench::value(Box::new(performable_queue()))]
fn queue_async_performables<S: std::borrow::BorrowMut<(Sender<u64>, Receiver<u64>)>>(mut owned_input: S) -> (u64, S) {
    let result = {
        let input = owned_input.borrow_mut();
        let input = black_box(input);
        ready(input.0.send_async(black_box(7))).expect(SEND_READY);
        black_box(ready(input.1.recv_async()).expect(RECEIVE_READY))
    };
    (result, owned_input)
}

#[metabench::benchmark(QUEUE_ASYNC_TOKIO, QUEUE_ASYNC, "tokio")]
#[bench::value(Box::new(tokio_queue()))]
fn queue_async_tokio<S: std::borrow::BorrowMut<(tokio::sync::mpsc::Sender<u64>, tokio::sync::mpsc::Receiver<u64>)>>(
    mut owned_input: S,
) -> (u64, S) {
    let result = {
        let input = owned_input.borrow_mut();
        let input = black_box(input);
        ready(input.0.send(black_box(7))).expect(SEND_READY);
        black_box(ready(input.1.recv()).expect(RECEIVE_READY))
    };
    (result, owned_input)
}

#[metabench::benchmark(UNBOUNDED_STD, UNBOUNDED, "std")]
#[bench::value(Box::new(standard_unbounded()))]
fn unbounded_std<S: std::borrow::BorrowMut<(mpsc::Sender<u64>, mpsc::Receiver<u64>)>>(mut owned_input: S) -> (u64, S) {
    let result = {
        let input = owned_input.borrow_mut();
        let input = black_box(input);
        input.0.send(black_box(7)).expect(SEND_READY);
        black_box(input.1.try_recv().expect(RECEIVE_READY))
    };
    (result, owned_input)
}

#[metabench::benchmark(UNBOUNDED_PERFORMABLES, UNBOUNDED, "performables")]
#[bench::value(Box::new(performable_unbounded()))]
fn unbounded_performables<S: std::borrow::BorrowMut<(Sender<u64>, Receiver<u64>)>>(mut owned_input: S) -> (u64, S) {
    let result = {
        let input = owned_input.borrow_mut();
        let input = black_box(input);
        input.0.try_send(black_box(7)).expect(SEND_READY);
        black_box(input.1.try_recv().expect(RECEIVE_READY))
    };
    (result, owned_input)
}

#[metabench::benchmark(UNBOUNDED_TOKIO, UNBOUNDED, "tokio")]
#[bench::value(Box::new(tokio_unbounded()))]
fn unbounded_tokio<S: std::borrow::BorrowMut<(tokio::sync::mpsc::UnboundedSender<u64>, tokio::sync::mpsc::UnboundedReceiver<u64>)>>(
    mut owned_input: S,
) -> (u64, S) {
    let result = {
        let input = owned_input.borrow_mut();
        let input = black_box(input);
        input.0.send(black_box(7)).expect(SEND_READY);
        black_box(input.1.try_recv().expect(RECEIVE_READY))
    };
    (result, owned_input)
}

#[metabench::benchmark(WATCH_READ_PERFORMABLES, WATCH_READ, "performables")]
#[bench::value(Box::new(channel::watch(7_u64)))]
fn watch_read_performables<S: std::borrow::Borrow<(WatchSender<u64>, WatchReceiver<u64>)>>(owned_input: S) -> (u64, S) {
    let result = {
        let input = owned_input.borrow();
        black_box(*black_box(input).1.borrow())
    };
    (result, owned_input)
}

#[metabench::benchmark(WATCH_READ_TOKIO, WATCH_READ, "tokio")]
#[bench::value(Box::new(tokio::sync::watch::channel(7_u64)))]
fn watch_read_tokio<S: std::borrow::Borrow<(tokio::sync::watch::Sender<u64>, tokio::sync::watch::Receiver<u64>)>>(
    owned_input: S,
) -> (u64, S) {
    let result = {
        let input = owned_input.borrow();
        black_box(*black_box(input).1.borrow())
    };
    (result, owned_input)
}

#[metabench::benchmark(WATCH_WRITE_PERFORMABLES, WATCH_WRITE, "performables")]
#[bench::value(Box::new(channel::watch(7_u64)))]
fn watch_write_performables<S: std::borrow::Borrow<(WatchSender<u64>, WatchReceiver<u64>)>>(owned_input: S) -> (u64, S) {
    let result = {
        let input = owned_input.borrow();
        black_box(black_box(input).0.send_replace(black_box(7)))
    };
    (result, owned_input)
}

#[metabench::benchmark(WATCH_WRITE_TOKIO, WATCH_WRITE, "tokio")]
#[bench::value(Box::new(tokio::sync::watch::channel(7_u64)))]
fn watch_write_tokio<S: std::borrow::Borrow<(tokio::sync::watch::Sender<u64>, tokio::sync::watch::Receiver<u64>)>>(
    owned_input: S,
) -> (u64, S) {
    let result = {
        let input = owned_input.borrow();
        black_box(black_box(input).0.send_replace(black_box(7)))
    };
    (result, owned_input)
}

#[metabench::benchmark(ONESHOT_PERFORMABLES, ONESHOT, "performables")]
#[bench::value(Box::new(performable_oneshot()))]
fn oneshot_performables<S: std::borrow::BorrowMut<OneshotReceiver<u64>>>(mut owned_input: S) -> (u64, S) {
    let result = {
        let value = owned_input.borrow_mut();
        black_box(
            black_box(value)
                .try_recv()
                .expect("setup delivered a value and retained this receiver"),
        )
    };
    (result, owned_input)
}

#[metabench::benchmark(ONESHOT_TOKIO, ONESHOT, "tokio")]
#[bench::value(Box::new(tokio_oneshot()))]
fn oneshot_tokio<S: std::borrow::BorrowMut<tokio::sync::oneshot::Receiver<u64>>>(mut owned_input: S) -> (u64, S) {
    let result = {
        let value = owned_input.borrow_mut();
        black_box(
            black_box(value)
                .try_recv()
                .expect("setup delivered a value and retained this receiver"),
        )
    };
    (result, owned_input)
}

fn ownership_and_initialization(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(ARC_CYCLE);
    let standard = std::sync::Arc::new(7_u64);
    let performable = Arc::new(7_u64);
    group.bench_function(ARC_STD.benchmark_name(), |b| b.iter(|| arc_std(&standard).0));
    group.bench_function(ARC_PERFORMABLES.benchmark_name(), |b| b.iter(|| arc_performables(&performable).0));
    group.finish();

    let mut group = criterion.benchmark_group(ONCE);
    let standard = std::sync::OnceLock::from(7_u64);
    let performable = OnceLock::from(7_u64);
    let tokio = tokio::sync::OnceCell::new_with(Some(7_u64));
    group.bench_function(ONCE_STD.benchmark_name(), |b| b.iter(|| once_std(&standard).0));
    group.bench_function(ONCE_PERFORMABLES.benchmark_name(), |b| b.iter(|| once_performables(&performable).0));
    group.bench_function(ONCE_TOKIO.benchmark_name(), |b| b.iter(|| once_tokio(&tokio).0));
    group.finish();

    let mut group = criterion.benchmark_group(LAZY);
    let standard = standard_lazy();
    let performable = performable_lazy();
    group.bench_function(LAZY_STD.benchmark_name(), |b| b.iter(|| lazy_std(&standard).0));
    group.bench_function(LAZY_PERFORMABLES.benchmark_name(), |b| b.iter(|| lazy_performables(&performable).0));
    group.finish();
}

fn barriers_and_notifications(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(BARRIER);
    let standard = std::sync::Barrier::new(1);
    let synchronous = Barrier::new(1);
    let asynchronous = Barrier::<Async>::new(1);
    let tokio = tokio::sync::Barrier::new(1);
    group.bench_function(BARRIER_STD.benchmark_name(), |b| b.iter(|| barrier_std(&standard).0));
    group.bench_function(BARRIER_SYNC.benchmark_name(), |b| b.iter(|| barrier_sync(&synchronous).0));
    group.bench_function(BARRIER_ASYNC.benchmark_name(), |b| b.iter(|| barrier_async(&asynchronous).0));
    group.bench_function(BARRIER_BLOCKING.benchmark_name(), |b| b.iter(|| barrier_blocking(&asynchronous).0));
    group.bench_function(BARRIER_TOKIO.benchmark_name(), |b| b.iter(|| barrier_tokio(&tokio).0));
    group.finish();

    let mut group = criterion.benchmark_group(NOTIFY);
    let standard = std::sync::Condvar::new();
    let synchronous = Condvar::new();
    let asynchronous = Condvar::<Async>::new();
    group.bench_function(NOTIFY_STD.benchmark_name(), |b| b.iter(|| notify_std(&standard).0));
    group.bench_function(NOTIFY_SYNC.benchmark_name(), |b| b.iter(|| notify_sync(&synchronous).0));
    group.bench_function(NOTIFY_ASYNC.benchmark_name(), |b| b.iter(|| notify_async(&asynchronous).0));
    group.finish();
}

fn queues(criterion: &mut Criterion) {
    let mut standard = standard_queue();
    let mut performable = performable_queue();
    let mut tokio = tokio_queue();
    let mut group = criterion.benchmark_group(QUEUE_TRY);
    group.bench_function(QUEUE_TRY_STD.benchmark_name(), |b| b.iter(|| queue_try_std(&mut standard).0));
    group.bench_function(QUEUE_TRY_PERFORMABLES.benchmark_name(), |b| {
        b.iter(|| queue_try_performables(&mut performable).0);
    });
    group.bench_function(QUEUE_TRY_TOKIO.benchmark_name(), |b| b.iter(|| queue_try_tokio(&mut tokio).0));
    group.finish();

    let mut group = criterion.benchmark_group(QUEUE_BLOCKING);
    let mut tokio_blocking = tokio_blocking_queue();
    group.bench_function(QUEUE_BLOCKING_STD.benchmark_name(), |b| {
        b.iter(|| queue_blocking_std(&mut standard).0);
    });
    group.bench_function(QUEUE_BLOCKING_PERFORMABLES.benchmark_name(), |b| {
        b.iter(|| queue_blocking_performables(&mut performable).0);
    });
    group.bench_function(QUEUE_BLOCKING_TOKIO.benchmark_name(), |b| {
        b.iter(|| queue_blocking_tokio(&mut tokio_blocking).0);
    });
    group.finish();

    let mut group = criterion.benchmark_group(QUEUE_ASYNC);
    group.bench_function(QUEUE_ASYNC_PERFORMABLES.benchmark_name(), |b| {
        b.iter(|| queue_async_performables(&mut performable).0);
    });
    group.bench_function(QUEUE_ASYNC_TOKIO.benchmark_name(), |b| b.iter(|| queue_async_tokio(&mut tokio).0));
    group.finish();

    let mut group = criterion.benchmark_group(UNBOUNDED);
    let mut standard = standard_unbounded();
    let mut performable = performable_unbounded();
    let mut tokio = tokio_unbounded();
    group.bench_function(UNBOUNDED_STD.benchmark_name(), |b| b.iter(|| unbounded_std(&mut standard).0));
    group.bench_function(UNBOUNDED_PERFORMABLES.benchmark_name(), |b| {
        b.iter(|| unbounded_performables(&mut performable).0);
    });
    group.bench_function(UNBOUNDED_TOKIO.benchmark_name(), |b| b.iter(|| unbounded_tokio(&mut tokio).0));
    group.finish();
}

fn watch_and_oneshot(criterion: &mut Criterion) {
    let performable = channel::watch(7_u64);
    let tokio = tokio::sync::watch::channel(7_u64);
    let mut group = criterion.benchmark_group(WATCH_READ);
    group.bench_function(WATCH_READ_PERFORMABLES.benchmark_name(), |b| {
        b.iter(|| watch_read_performables(&performable).0);
    });
    group.bench_function(WATCH_READ_TOKIO.benchmark_name(), |b| b.iter(|| watch_read_tokio(&tokio).0));
    group.finish();

    let mut group = criterion.benchmark_group(WATCH_WRITE);
    group.bench_function(WATCH_WRITE_PERFORMABLES.benchmark_name(), |b| {
        b.iter(|| watch_write_performables(&performable).0);
    });
    group.bench_function(WATCH_WRITE_TOKIO.benchmark_name(), |b| b.iter(|| watch_write_tokio(&tokio).0));
    group.finish();

    let mut group = criterion.benchmark_group(ONESHOT);
    group.bench_function(ONESHOT_PERFORMABLES.benchmark_name(), |b| {
        b.iter_batched_ref(
            performable_oneshot,
            |receiver| oneshot_performables(receiver).0,
            BatchSize::SmallInput,
        );
    });
    group.bench_function(ONESHOT_TOKIO.benchmark_name(), |b| {
        b.iter_batched_ref(tokio_oneshot, |receiver| oneshot_tokio(receiver).0, BatchSize::SmallInput);
    });
    group.finish();
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    #[cfg(feature = "seismograph")]
    seismograph::recorder(black_box(Configuration::default()));

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
    group.bench_function(PERMIT_SYNC.benchmark_name(), |b| b.iter(|| permit_coalesced_sync(&sync).0));
    group.bench_function(PERMIT_ASYNC.benchmark_name(), |b| {
        b.iter(|| permit_coalesced_async(&asynchronous).0);
    });
    group.bench_function(PERMIT_STD.benchmark_name(), |b| b.iter(|| permit_coalesced_std(&std).0));
    group.bench_function(PERMIT_PREVIOUS.benchmark_name(), |b| {
        b.iter(|| permit_coalesced_previous(&previous).0);
    });
    group.bench_function(PERMIT_TOKIO.benchmark_name(), |b| b.iter(|| permit_coalesced_tokio(&tokio).0));
    group.finish();
    let mut asynchronous = async_notification();
    let mut tokio = tokio_notification();
    let mut group = criterion.benchmark_group(PERMIT_CYCLE);
    group.bench_function(PERMIT_CYCLE_SYNC.benchmark_name(), |b| b.iter(|| permit_cycle_sync(&sync).0));
    group.bench_function(PERMIT_CYCLE_ASYNC.benchmark_name(), |b| {
        b.iter(|| permit_cycle_async(&mut asynchronous).0);
    });
    group.bench_function(PERMIT_CYCLE_STD.benchmark_name(), |b| b.iter(|| permit_cycle_std(&std).0));
    group.bench_function(PERMIT_CYCLE_PREVIOUS.benchmark_name(), |b| {
        b.iter(|| permit_cycle_previous(&previous).0);
    });
    group.bench_function(PERMIT_CYCLE_TOKIO.benchmark_name(), |b| b.iter(|| permit_cycle_tokio(&mut tokio).0));
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
