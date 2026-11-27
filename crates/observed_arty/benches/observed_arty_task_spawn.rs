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
use std::fmt;
use std::hint::black_box;
use std::time::Duration;

use arty::runtime::Runtime;
use arty::runtime::policy::{MonitorPolicy, WorkersPolicy};
use arty::task::JoinHandle;
use criterion::{BenchmarkId, Criterion, Throughput};
use metabench::benchmark;
use observed::Sink;
use observed_arty::ObservedExt as _;
use tick::SimpleClock;

/// Keeps a remote batch large enough to amortize scheduler coordination.
const BATCH_SIZE: usize = 64;
/// Repeats each remote batch enough times for a stable throughput sample.
const BATCHES: usize = 64;
const TASK_COUNT: usize = BATCH_SIZE * BATCHES;

/// Selects the single-worker or cross-worker task submission workload.
#[derive(Clone, Copy, Debug)]
enum Workload {
    Local,
    Remote,
}

impl Workload {
    /// Names the workload in Criterion and metabench output.
    const fn name(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
        }
    }

    /// Supplies the worker count that defines the selected workload.
    const fn workers(self) -> usize {
        match self {
            Self::Local => 1,
            Self::Remote => 4,
        }
    }
}

/// Retains the runtime and reusable storage for one measured workload.
struct Case {
    runtime: Runtime,
    /// Reuses one remote batch allocation instead of measuring vector growth.
    handles: Vec<JoinHandle<()>>,
    workload: Workload,
    /// Reads monotonic benchmark time through the shared clock abstraction.
    clock: SimpleClock,
}

impl fmt::Debug for Case {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Case")
            .field("workload", &self.workload)
            .field("handle_count", &self.handles.len())
            .finish_non_exhaustive()
    }
}

impl Case {
    /// Builds and warms one runtime before Criterion measures repeated work.
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
            clock: SimpleClock::new_system(),
        };
        black_box(case.run());
        case
    }

    /// Runs one local or remote workload and returns its elapsed time.
    fn run(&mut self) -> Duration {
        match self.workload {
            Workload::Local => {
                let parent = self.runtime.scheduler().spawn_anywhere(self.clock.clone(), |cx, clock| async move {
                    let start = clock.instant();
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
                let start = self.clock.instant();
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

/// Matches the Arty benchmark suite's sampling window for comparable runs.
fn configured_criterion() -> Criterion {
    Criterion::default()
        .sample_size(50)
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(2))
}

/// Registers local and remote cases with and without the integration.
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
/// Measures one complete task-spawn workload.
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
