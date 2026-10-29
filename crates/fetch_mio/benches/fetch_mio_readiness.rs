// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compares boxed readiness futures with direct readiness polling.

#![allow(missing_docs, reason = "benchmark code")]
#![expect(
    clippy::panic,
    clippy::unwrap_used,
    reason = "benchmark fixture failures provide sufficient context"
)]

use std::hint::black_box;
use std::io::{self, IoSlice};
use std::mem::MaybeUninit;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use criterion::{BatchSize, Criterion};
use hyper::rt::ReadBuf;

#[expect(dead_code, unreachable_pub, reason = "the benchmark reuses the production stream core")]
#[path = "../src/stream.rs"]
mod production;

use production::{StreamCore, StreamIo};

const READ_GROUP: &str = "fetch_mio_readiness/read";
const WRITE_GROUP: &str = "fetch_mio_readiness/write";

type ReadinessFuture = Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'static>>;

struct ReadInput<C> {
    core: C,
    signal: Signal,
    destination: [u8; 1],
}

struct WriteInput<C> {
    core: C,
    signal: Signal,
}

struct ReadOutput<C> {
    input: ReadInput<C>,
    pending_polls: usize,
}

struct WriteOutput<C> {
    input: WriteInput<C>,
    pending_polls: usize,
    bytes_written: usize,
}

fn boxed_read_input() -> ReadInput<BoxedCore<ScriptedIo>> {
    let io = ScriptedIo::read();
    let signal = io.readable.clone();
    ReadInput {
        core: BoxedCore::new(io),
        signal,
        destination: [0],
    }
}

fn direct_read_input() -> ReadInput<StreamCore<ScriptedIo>> {
    let io = ScriptedIo::read();
    let signal = io.readable.clone();
    ReadInput {
        core: StreamCore::new(io),
        signal,
        destination: [0],
    }
}

fn boxed_write_input() -> WriteInput<BoxedCore<ScriptedIo>> {
    let io = ScriptedIo::write();
    let signal = io.writable.clone();
    WriteInput {
        core: BoxedCore::new(io),
        signal,
    }
}

fn direct_write_input() -> WriteInput<StreamCore<ScriptedIo>> {
    let io = ScriptedIo::write();
    let signal = io.writable.clone();
    WriteInput {
        core: StreamCore::new(io),
        signal,
    }
}

#[metabench::benchmark(READ_BOXED, READ_GROUP, "boxed_future")]
#[bench::would_block_ready_retry(boxed_read_input())]
fn read_boxed(mut input: ReadInput<BoxedCore<ScriptedIo>>) -> ReadOutput<BoxedCore<ScriptedIo>> {
    {
        let mut buffer = ReadBuf::new(&mut input.destination);
        let mut context = Context::from_waker(Waker::noop());
        assert!(input.core.poll_read(&mut context, buffer.unfilled()).is_pending());
        input.signal.signal();
        assert!(input.core.poll_read(&mut context, buffer.unfilled()).is_ready());
        assert_eq!(buffer.filled(), &[42]);
    }
    ReadOutput { input, pending_polls: 1 }
}

#[metabench::benchmark(READ_DIRECT, READ_GROUP, "direct_poll")]
#[bench::would_block_ready_retry(direct_read_input())]
fn read_direct(mut input: ReadInput<StreamCore<ScriptedIo>>) -> ReadOutput<StreamCore<ScriptedIo>> {
    {
        let mut buffer = ReadBuf::new(&mut input.destination);
        let mut context = Context::from_waker(Waker::noop());
        assert!(input.core.poll_read(&mut context, buffer.unfilled()).is_pending());
        input.signal.signal();
        assert!(input.core.poll_read(&mut context, buffer.unfilled()).is_ready());
        assert_eq!(buffer.filled(), &[42]);
    }
    ReadOutput { input, pending_polls: 1 }
}

#[metabench::benchmark(WRITE_BOXED, WRITE_GROUP, "boxed_future")]
#[bench::would_block_ready_retry(boxed_write_input())]
fn write_boxed(mut input: WriteInput<BoxedCore<ScriptedIo>>) -> WriteOutput<BoxedCore<ScriptedIo>> {
    let mut context = Context::from_waker(Waker::noop());
    assert!(input.core.poll_write(&mut context, &[42]).is_pending());
    input.signal.signal();
    let Poll::Ready(Ok(bytes_written)) = input.core.poll_write(&mut context, &[42]) else {
        panic!("the scripted write must complete after readiness");
    };
    WriteOutput {
        input,
        pending_polls: 1,
        bytes_written,
    }
}

#[metabench::benchmark(WRITE_DIRECT, WRITE_GROUP, "direct_poll")]
#[bench::would_block_ready_retry(direct_write_input())]
fn write_direct(mut input: WriteInput<StreamCore<ScriptedIo>>) -> WriteOutput<StreamCore<ScriptedIo>> {
    let mut context = Context::from_waker(Waker::noop());
    assert!(input.core.poll_write(&mut context, &[42]).is_pending());
    input.signal.signal();
    let Poll::Ready(Ok(bytes_written)) = input.core.poll_write(&mut context, &[42]) else {
        panic!("the scripted write must complete after readiness");
    };
    WriteOutput {
        input,
        pending_polls: 1,
        bytes_written,
    }
}

#[metabench::benchmark(WRITE_VECTORED_DIRECT, WRITE_GROUP, "direct_poll_vectored")]
#[bench::would_block_ready_retry(direct_write_input())]
fn write_vectored_direct(mut input: WriteInput<StreamCore<ScriptedIo>>) -> WriteOutput<StreamCore<ScriptedIo>> {
    let mut context = Context::from_waker(Waker::noop());
    let buffers = [IoSlice::new(&[42])];
    assert!(input.core.poll_write_vectored(&mut context, &buffers).is_pending());
    input.signal.signal();
    let Poll::Ready(Ok(bytes_written)) = input.core.poll_write_vectored(&mut context, &buffers) else {
        panic!("the scripted vectored write must complete after readiness");
    };
    WriteOutput {
        input,
        pending_polls: 1,
        bytes_written,
    }
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    let mut read = criterion.benchmark_group(READ_GROUP);
    read.bench_function(READ_BOXED.benchmark_name(), |bencher| {
        bencher.iter_batched(
            boxed_read_input,
            |input| {
                let output = read_boxed(input);
                black_box(output.pending_polls);
                output.input
            },
            BatchSize::PerIteration,
        );
    });
    read.bench_function(READ_DIRECT.benchmark_name(), |bencher| {
        bencher.iter_batched(
            direct_read_input,
            |input| {
                let output = read_direct(input);
                black_box(output.pending_polls);
                output.input
            },
            BatchSize::PerIteration,
        );
    });
    read.finish();

    let mut write = criterion.benchmark_group(WRITE_GROUP);
    write.bench_function(WRITE_BOXED.benchmark_name(), |bencher| {
        bencher.iter_batched(
            boxed_write_input,
            |input| {
                let output = write_boxed(input);
                black_box((output.pending_polls, output.bytes_written));
                output.input
            },
            BatchSize::PerIteration,
        );
    });
    write.bench_function(WRITE_DIRECT.benchmark_name(), |bencher| {
        bencher.iter_batched(
            direct_write_input,
            |input| {
                let output = write_direct(input);
                black_box((output.pending_polls, output.bytes_written));
                output.input
            },
            BatchSize::PerIteration,
        );
    });
    write.bench_function(WRITE_VECTORED_DIRECT.benchmark_name(), |bencher| {
        bencher.iter_batched(
            direct_write_input,
            |input| {
                let output = write_vectored_direct(input);
                black_box((output.pending_polls, output.bytes_written));
                output.input
            },
            BatchSize::PerIteration,
        );
    });
    write.finish();
}

struct BoxedCore<I> {
    io: I,
    read_wait: Option<ReadinessFuture>,
    write_wait: Option<ReadinessFuture>,
}

impl<I> BoxedCore<I> {
    fn new(io: I) -> Self {
        Self {
            io,
            read_wait: None,
            write_wait: None,
        }
    }
}

impl BoxedCore<ScriptedIo> {
    fn poll_read(&mut self, context: &mut Context<'_>, mut buffer: hyper::rt::ReadBufCursor<'_>) -> Poll<io::Result<()>> {
        loop {
            // SAFETY: `ScriptedIo::read` initializes the returned byte count.
            match self.io.read(unsafe { buffer.as_mut() }) {
                Ok(length) => {
                    self.read_wait = None;
                    // SAFETY: `ScriptedIo::read` initialized `length` bytes.
                    unsafe {
                        buffer.advance(length);
                    }
                    return Poll::Ready(Ok(()));
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => {
                    self.read_wait = None;
                    return Poll::Ready(Err(error));
                }
            }

            let future = self.read_wait.get_or_insert_with(|| self.io.readiness(Direction::Read));
            match future.as_mut().poll(context) {
                Poll::Ready(result) => {
                    self.read_wait = None;
                    result?;
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }

    fn poll_write(&mut self, context: &mut Context<'_>, buffer: &[u8]) -> Poll<io::Result<usize>> {
        loop {
            match self.io.write(buffer) {
                Ok(length) => {
                    self.write_wait = None;
                    return Poll::Ready(Ok(length));
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => {
                    self.write_wait = None;
                    return Poll::Ready(Err(error));
                }
            }

            let future = self.write_wait.get_or_insert_with(|| self.io.readiness(Direction::Write));
            match future.as_mut().poll(context) {
                Poll::Ready(result) => {
                    self.write_wait = None;
                    result?;
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

struct ScriptedIo {
    read_calls: usize,
    write_calls: usize,
    readable: Signal,
    writable: Signal,
}

impl ScriptedIo {
    fn read() -> Self {
        Self {
            read_calls: 0,
            write_calls: 2,
            readable: Signal::default(),
            writable: Signal::default(),
        }
    }

    fn write() -> Self {
        Self {
            read_calls: 2,
            write_calls: 0,
            readable: Signal::default(),
            writable: Signal::default(),
        }
    }

    fn readiness(&self, direction: Direction) -> ReadinessFuture {
        let signal = match direction {
            Direction::Read => self.readable.clone(),
            Direction::Write => self.writable.clone(),
        };
        Box::pin(SignalFuture { signal })
    }
}

impl StreamIo for ScriptedIo {
    fn read(&mut self, buffer: &mut [MaybeUninit<u8>]) -> io::Result<usize> {
        self.read_calls += 1;
        if self.read_calls == 1 {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        buffer[0].write(42);
        Ok(1)
    }

    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.write_calls += 1;
        if self.write_calls == 1 {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        Ok(buffer.len())
    }

    fn write_vectored(&mut self, buffers: &[IoSlice<'_>]) -> io::Result<usize> {
        self.write(buffers.first().map_or(&[], |buffer| &**buffer))
    }

    fn shutdown(&self) -> io::Result<()> {
        Ok(())
    }

    fn poll_read_ready(&self, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.readable.poll_ready(context)
    }

    fn poll_write_ready(&self, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.writable.poll_ready(context)
    }
}

#[derive(Clone, Default)]
struct Signal {
    state: Arc<Mutex<SignalState>>,
}

#[derive(Default)]
struct SignalState {
    ready: bool,
    waker: Option<Waker>,
}

impl Signal {
    fn poll_ready(&self, context: &Context<'_>) -> Poll<io::Result<()>> {
        let mut state = self.state.lock().unwrap();
        if std::mem::take(&mut state.ready) {
            return Poll::Ready(Ok(()));
        }
        if state.waker.as_ref().is_none_or(|waker| !waker.will_wake(context.waker())) {
            state.waker = Some(context.waker().clone());
        }
        Poll::Pending
    }

    fn signal(&self) {
        let waker = {
            let mut state = self.state.lock().unwrap();
            state.ready = true;
            state.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

struct SignalFuture {
    signal: Signal,
}

impl Future for SignalFuture {
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.signal.poll_ready(cx)
    }
}

#[derive(Clone, Copy)]
enum Direction {
    Read,
    Write,
}

metabench::main!(
    criterion = criterion_benchmarks,
    benchmarks = [READ_BOXED, READ_DIRECT, WRITE_BOXED, WRITE_DIRECT, WRITE_VECTORED_DIRECT],
    allocator = std::alloc::System,
);
