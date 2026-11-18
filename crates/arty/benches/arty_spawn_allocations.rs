// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Measures warmed task spawning with Metabench.
//!
//! Run with `--criterion --allocations`. Metabench runs allocation cases in
//! separate processes. To exclude monitoring dependencies, use a separate
//! package with Arty as a normal, runtime-only dependency.
//! Arty's own dev-dependencies can enable monitoring through feature unification.
//! Runtime setup and warm-up are excluded.
//! Unrelated process threads can contribute allocations. The local case measures
//! 4096 sequential children in one parent. The remote case measures 64 batches
//! of 64 tasks on four workers, including dispatch and joins. The blocking case
//! uses the same batches and one blocking worker. Reports show totals per
//! 4096-task workload, not per task.
//! The blocking backlog case queues 4096 tasks behind a gate. Its timer covers
//! releasing the gate and joining all tasks, but excludes submission.
//! Its allocation counts include the gate and its task.

use std::borrow::BorrowMut;
use std::hint::black_box;
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use arty::runtime::{BlockingPoolPolicy, MonitorPolicy, Runtime, WorkersPolicy};
use arty::task::JoinHandle;
use criterion::{BenchmarkId, Criterion, Throughput};
use metabench::benchmark;

#[derive(Clone, Copy, Debug)]
enum Workload {
    Local,
    Remote,
    Blocking,
    BlockingBacklog,
}

impl Workload {
    const fn name(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
            Self::Blocking => "blocking",
            Self::BlockingBacklog => "blocking_backlog",
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
    fn new(workload: Workload, enabled: bool) -> Self {
        let runtime = Runtime::builder()
            .workers(WorkersPolicy::exactly(if matches!(workload, Workload::Local) { 1 } else { 4 }))
            .blocking_pool(BlockingPoolPolicy::shared().max(1))
            .monitor(if enabled {
                MonitorPolicy::enabled()
            } else {
                MonitorPolicy::disabled()
            })
            .build()
            .expect("benchmark requires the configured worker count");
        let mut case = Self {
            runtime,
            handles: Vec::with_capacity(if matches!(workload, Workload::BlockingBacklog) { 4096 } else { 64 }),
            workload,
        };
        black_box(case.run());
        case
    }

    fn run(&mut self) -> Duration {
        if matches!(self.workload, Workload::BlockingBacklog) {
            self.handles.clear();
            let gate = Arc::new(Barrier::new(2));
            let worker_gate = Arc::clone(&gate);
            let first = self.runtime.scheduler().spawn_blocking(move || {
                worker_gate.wait();
            });
            for _ in 0..4096 {
                self.handles.push(self.runtime.scheduler().spawn_blocking(|| black_box(())));
            }
            let start = Instant::now();
            gate.wait();
            futures::executor::block_on(first).expect("benchmark gate succeeds");
            for task in &mut self.handles {
                futures::executor::block_on(task).expect("benchmark task succeeds");
            }
            return start.elapsed();
        }
        let start = Instant::now();
        match self.workload {
            Workload::Local => {
                let parent = self.runtime.scheduler().spawn_anywhere((), |cx, ()| async move {
                    for _ in 0..4096 {
                        cx.scheduler()
                            .spawn(async |_| black_box(()))
                            .await
                            .expect("benchmark child succeeds");
                    }
                });
                futures::executor::block_on(parent).expect("benchmark parent succeeds");
            }
            Workload::Remote | Workload::Blocking | Workload::BlockingBacklog => {
                for _ in 0..64 {
                    self.handles.clear();
                    for _ in 0..64 {
                        let task = match self.workload {
                            Workload::Blocking => self.runtime.scheduler().spawn_blocking(|| black_box(())),
                            Workload::Remote => self.runtime.scheduler().spawn_anywhere((), |_, ()| async { black_box(()) }),
                            Workload::Local | Workload::BlockingBacklog => unreachable!("local and backlog spawning use separate paths"),
                        };
                        self.handles.push(task);
                    }
                    for task in &mut self.handles {
                        futures::executor::block_on(task).expect("benchmark task succeeds");
                    }
                }
            }
        }
        start.elapsed()
    }
}

fn configured_criterion() -> Criterion {
    Criterion::default()
        .sample_size(50)
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(2))
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(SPAWN.group_name());
    group.throughput(Throughput::Elements(4096));
    for workload in [Workload::Local, Workload::Remote, Workload::Blocking, Workload::BlockingBacklog] {
        for enabled in [true, false] {
            let mode = if enabled { "idle" } else { "disabled" };
            group.bench_function(
                BenchmarkId::new(SPAWN.benchmark_name(), format!("{}_{mode}", workload.name())),
                |bencher| {
                    let mut case = Case::new(workload, enabled);
                    bencher.iter_custom(|iterations| {
                        let mut elapsed = Duration::ZERO;
                        for _ in 0..iterations {
                            elapsed += spawn_workload(&mut case).0;
                        }
                        elapsed
                    });
                },
            );
        }
    }
    group.finish();
}

#[benchmark(SPAWN, "arty_spawn_allocations/tasks", "spawn")]
#[bench::local_idle(Box::new(Case::new(Workload::Local, true)))]
#[bench::local_disabled(Box::new(Case::new(Workload::Local, false)))]
#[bench::remote_idle(Box::new(Case::new(Workload::Remote, true)))]
#[bench::remote_disabled(Box::new(Case::new(Workload::Remote, false)))]
#[bench::blocking_idle(Box::new(Case::new(Workload::Blocking, true)))]
#[bench::blocking_disabled(Box::new(Case::new(Workload::Blocking, false)))]
#[bench::blocking_backlog_idle(Box::new(Case::new(Workload::BlockingBacklog, true)))]
#[bench::blocking_backlog_disabled(Box::new(Case::new(Workload::BlockingBacklog, false)))]
fn spawn_workload<S: BorrowMut<Case>>(mut input: S) -> (Duration, S) {
    let elapsed = input.borrow_mut().run();
    (elapsed, input)
}

metabench::main!(
    criterion = {
        factory = configured_criterion,
        benchmarks = criterion_benchmarks,
        unit = "ns",
    },
    benchmarks = [SPAWN],
);
