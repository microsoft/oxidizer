// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! One bounded workload shared by native time measurements and process-wide Callgrind.

use std::hint::black_box;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::Poll;
use std::time::{Duration, Instant};

use arty::runtime::{BlockingPoolPolicy, Runtime, WorkersPolicy};
use gungraun::client_requests::callgrind;
use nix::time::ClockId;

pub(super) type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Monitoring {
    Off,
    State,
    Timing,
}

#[derive(Clone, Copy, Debug)]
enum Engine {
    Arty,
    Tokio,
}

#[derive(Clone, Copy, Debug)]
enum Workload {
    Spawn,
    Yield,
}

#[derive(Clone, Copy, Debug)]
struct Options {
    engine: Engine,
    workers: usize,
    batch_size: usize,
    batches: usize,
    monitoring: Monitoring,
    workload: Workload,
    callgrind: bool,
}

impl Options {
    fn parse() -> Result<Self> {
        let mut options = Self {
            engine: Engine::Arty,
            workers: 1,
            batch_size: 20,
            batches: 1000,
            monitoring: Monitoring::Off,
            workload: Workload::Spawn,
            callgrind: false,
        };
        let mut args = std::env::args().skip(1);
        while let Some(argument) = args.next() {
            if argument == "--callgrind" {
                options.callgrind = true;
                continue;
            }
            if argument == "--bench" {
                continue;
            }
            let value = args.next().ok_or_else(|| invalid(&format!("missing value for {argument}")))?;
            match argument.as_str() {
                "--runtime" => {
                    options.engine = match value.as_str() {
                        "arty" => Engine::Arty,
                        "tokio" => Engine::Tokio,
                        _ => return Err(invalid("runtime must be arty or tokio").into()),
                    };
                }
                "--workers" => options.workers = value.parse()?,
                "--batch-size" => options.batch_size = value.parse()?,
                "--batches" => options.batches = value.parse()?,
                "--monitor" => {
                    options.monitoring = match value.as_str() {
                        "off" => Monitoring::Off,
                        "state" => Monitoring::State,
                        "timing" => Monitoring::Timing,
                        _ => return Err(invalid("monitor must be off, state, or timing").into()),
                    };
                }
                "--workload" => {
                    options.workload = match value.as_str() {
                        "spawn" => Workload::Spawn,
                        "yield" => Workload::Yield,
                        _ => return Err(invalid("workload must be spawn or yield").into()),
                    };
                }
                _ => return Err(invalid(&format!("unknown argument {argument}")).into()),
            }
        }
        if options.workers == 0 || options.workers > 64 || options.batch_size == 0 || options.batch_size > 100_000 {
            return Err(invalid("workers must be 1..=64 and batch size 1..=100000").into());
        }
        if options.batches == 0 || options.batches > 10_000_000 {
            return Err(invalid("batches must be 1..=10000000").into());
        }
        if matches!(options.engine, Engine::Tokio) && options.monitoring != Monitoring::Off {
            return Err(invalid("live monitor modes apply only to Arty").into());
        }
        Ok(options)
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

pub(super) fn run<G>(start_monitor: impl FnOnce(&Runtime, Monitoring, Arc<AtomicU64>) -> Result<G>) -> Result<()> {
    let options = Options::parse()?;
    seismograph::recorder(seismograph::recorder::Configuration::default());
    let monitor_batches = Arc::new(AtomicU64::new(0));
    let mut case = Case::new(options)?;
    let monitor = match &case {
        Case::Arty { runtime, .. } => Some(start_monitor(runtime, options.monitoring, Arc::clone(&monitor_batches))?),
        Case::Tokio { .. } => None,
    };
    for _ in 0..64 {
        case.batch(options)?;
    }
    let before_monitor_batches = monitor_batches.load(Ordering::Relaxed);
    if options.callgrind {
        // Unlike a function's collection toggle, instrumentation applies process-wide.
        // Each thread keeps collection enabled so already-existing workers contribute.
        callgrind::start_instrumentation();
    }
    let cpu_start = process_cpu_time()?;
    let start = Instant::now();
    for _ in 0..options.batches {
        case.batch(options)?;
    }
    let wall = start.elapsed();
    let cpu = process_cpu_time()?
        .checked_sub(cpu_start)
        .ok_or_else(|| invalid("process CPU clock moved backward"))?;
    if options.callgrind {
        callgrind::stop_instrumentation();
    }
    let callbacks = monitor_batches.load(Ordering::Relaxed).saturating_sub(before_monitor_batches);
    drop(monitor);
    drop(case);
    let tasks = options
        .batches
        .checked_mul(options.batch_size)
        .ok_or_else(|| invalid("task count overflow"))?;
    println!(
        "{}",
        serde_json::json!({
            "runtime": match options.engine { Engine::Arty => "arty", Engine::Tokio => "tokio" },
            "monitor": match options.monitoring { Monitoring::Off => "off", Monitoring::State => "state", Monitoring::Timing => "timing" },
            "workload": match options.workload { Workload::Spawn => "spawn", Workload::Yield => "yield" },
            "workers": options.workers,
            "batch_size": options.batch_size,
            "batches": options.batches,
            "tasks": tasks,
            "wall_nanos": wall.as_nanos(),
            "cpu_nanos": cpu.as_nanos(),
            "monitor_batches": callbacks,
            "under_callgrind": options.callgrind,
        })
    );
    Ok(())
}

fn process_cpu_time() -> Result<Duration> {
    let time = ClockId::CLOCK_PROCESS_CPUTIME_ID.now()?;
    Ok(Duration::new(u64::try_from(time.tv_sec())?, u32::try_from(time.tv_nsec())?))
}

enum Case {
    Arty {
        runtime: Runtime,
        handles: Vec<arty::task::JoinHandle<()>>,
    },
    Tokio {
        runtime: tokio::runtime::Runtime,
        handles: Vec<tokio::task::JoinHandle<()>>,
    },
}

impl Case {
    fn new(options: Options) -> Result<Self> {
        match options.engine {
            Engine::Arty => Ok(Self::Arty {
                runtime: Runtime::builder()
                    .workers(WorkersPolicy::exactly(options.workers))
                    .blocking_pool(BlockingPoolPolicy::shared().max(1))
                    .build()?,
                handles: Vec::with_capacity(options.batch_size),
            }),
            Engine::Tokio => Ok(Self::Tokio {
                runtime: tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(options.workers)
                    .max_blocking_threads(1)
                    .enable_time()
                    .build()?,
                handles: Vec::with_capacity(options.batch_size),
            }),
        }
    }

    fn batch(&mut self, options: Options) -> Result<()> {
        match self {
            Self::Arty { runtime, handles } => {
                handles.clear();
                handles.extend((0..options.batch_size).map(|_| {
                    runtime
                        .scheduler()
                        .spawn_anywhere(matches!(options.workload, Workload::Yield), |_, should_yield| async move {
                            payload(should_yield).await;
                        })
                }));
                for handle in &mut *handles {
                    black_box(futures::executor::block_on(handle)?);
                }
            }
            Self::Tokio { runtime, handles } => {
                handles.clear();
                handles.extend((0..options.batch_size).map(|_| runtime.spawn(payload(matches!(options.workload, Workload::Yield)))));
                for handle in &mut *handles {
                    black_box(futures::executor::block_on(handle)?);
                }
            }
        }
        Ok(())
    }
}

async fn payload(should_yield: bool) {
    if should_yield {
        let mut first = true;
        std::future::poll_fn(move |context| {
            if std::mem::replace(&mut first, false) {
                context.waker().wake_by_ref();
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        })
        .await;
    }
    black_box(());
}
