// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compares Arty task spawning with Observed disabled and enabled.
//!
//! Runtime setup and warm-up are outside measurement. Each workload measures
//! 4096 tasks. The local workload spawns and awaits sequential children on one
//! worker. The remote workload submits 64 batches of 64 tasks to four workers.
//! Use Criterion for the wall-clock comparison. Native probes include
//! multithreaded scheduler activity, so their instruction counts are not
//! isolated task-spawn costs.

use std::borrow::BorrowMut;
use std::hint::black_box;
use std::time::{Duration, Instant};

use arty::runtime::Runtime;
use arty::runtime::policy::{MonitorPolicy, WorkersPolicy};
use arty::task::JoinHandle;
use criterion::{BenchmarkId, Criterion, Throughput};
use metabench::benchmark;
use observed::Sink;
use observed_arty::ObservedExt as _;

const BATCH_SIZE: usize = 64;
const BATCHES: usize = 64;
const TASK_COUNT: usize = BATCH_SIZE * BATCHES;

#[derive(Clone, Copy, Debug)]
enum Workload {
    Local,
    Remote,
}

impl Workload {
    const fn name(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
        }
    }

    const fn workers(self) -> usize {
        match self {
            Self::Local => 1,
            Self::Remote => 4,
        }
    }
}

#[derive(Debug)]
struct Case {
    runtime: Runtime,
    handles: Vec<JoinHandle<()>>,
    workload: Workload,
}

impl Case {
    fn new(workload: Workload, observed_enabled: bool) -> Self {
        let builder = Runtime::builder()
            .workers(WorkersPolicy::exactly(workload.workers()))
            .monitor(MonitorPolicy::disabled());
        let runtime = if observed_enabled {
            builder.enable_observed(Sink::builder("observed_arty_task_spawn").build()).build()
        } else {
            builder.build()
        }
        .expect("benchmark requires the configured worker count");
        let mut case = Self {
            runtime,
            handles: Vec::with_capacity(BATCH_SIZE),
            workload,
        };
        black_box(case.run());
        case
    }

    fn run(&mut self) -> Duration {
        match self.workload {
            Workload::Local => {
                let parent = self.runtime.scheduler().spawn_anywhere((), |cx, ()| async move {
                    let start = Instant::now();
                    for _ in 0..TASK_COUNT {
                        cx.scheduler()
                            .spawn(async |_| black_box(()))
                            .await
                            .expect("benchmark child succeeds");
                    }
                    start.elapsed()
                });
                futures::executor::block_on(parent).expect("benchmark parent succeeds")
            }
            Workload::Remote => {
                let start = Instant::now();
                for _ in 0..BATCHES {
                    self.handles.clear();
                    self.handles
                        .extend((0..BATCH_SIZE).map(|_| self.runtime.scheduler().spawn_anywhere((), |_, ()| async { black_box(()) })));
                    for task in &mut self.handles {
                        futures::executor::block_on(task).expect("benchmark task succeeds");
                    }
                }
                start.elapsed()
            }
        }
    }
}

fn configured_criterion() -> Criterion {
    Criterion::default()
        .sample_size(50)
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(2))
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(TASK_SPAWN.group_name());
    group.throughput(Throughput::Elements(TASK_COUNT as u64));
    for workload in [Workload::Local, Workload::Remote] {
        for observed_enabled in [false, true] {
            let integration = if observed_enabled { "observed" } else { "disabled" };
            group.bench_function(
                BenchmarkId::new(TASK_SPAWN.benchmark_name(), format!("{}_{integration}", workload.name())),
                |bencher| {
                    let mut case = Case::new(workload, observed_enabled);
                    bencher.iter_custom(|iterations| {
                        let mut elapsed = Duration::ZERO;
                        for _ in 0..iterations {
                            elapsed += task_spawn(&mut case).0;
                        }
                        elapsed
                    });
                },
            );
        }
    }
    group.finish();
}

#[benchmark(TASK_SPAWN, "observed_arty_task_spawn/tasks", "spawn")]
#[bench::local_disabled(Box::new(Case::new(Workload::Local, false)))]
#[bench::local_observed(Box::new(Case::new(Workload::Local, true)))]
#[bench::remote_disabled(Box::new(Case::new(Workload::Remote, false)))]
#[bench::remote_observed(Box::new(Case::new(Workload::Remote, true)))]
fn task_spawn<S: BorrowMut<Case>>(mut input: S) -> (Duration, S) {
    let elapsed = input.borrow_mut().run();
    (elapsed, input)
}

metabench::main!(
    criterion = {
        factory = configured_criterion,
        benchmarks = criterion_benchmarks,
        unit = "ns",
    },
    benchmarks = [TASK_SPAWN],
);
