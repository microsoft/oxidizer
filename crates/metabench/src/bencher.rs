// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::hint::black_box;
use std::marker::PhantomData;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::time::{Duration, Instant};

use alloc_tracker::Operation;

pub(crate) trait MeasurementControl {
    fn start(&mut self) -> Result<(), crate::Error>;
    fn stop(&mut self) -> Result<(), crate::Error>;
}

pub(crate) enum Backend<'borrow, 'measurement> {
    Criterion(&'borrow mut criterion::Bencher<'measurement>),
    Allocations {
        operation: &'borrow Operation,
        samples: u64,
    },
    Direct {
        iterations: u64,
        control: &'borrow mut dyn MeasurementControl,
    },
    Gungraun,
}

/// A backend-independent benchmark driver.
pub struct Bencher<'borrow, 'measurement> {
    pub(crate) backend: Backend<'borrow, 'measurement>,
    runs: u8,
    failure: Option<crate::Error>,
}

impl<'borrow, 'measurement> Bencher<'borrow, 'measurement> {
    pub(crate) fn criterion(bencher: &'borrow mut criterion::Bencher<'measurement>) -> Self {
        Self {
            backend: Backend::Criterion(bencher),
            runs: 0,
            failure: None,
        }
    }

    pub(crate) const fn allocations(operation: &'borrow Operation, samples: u64) -> Self {
        Self {
            backend: Backend::Allocations { operation, samples },
            runs: 0,
            failure: None,
        }
    }

    pub(crate) fn direct(iterations: u64, control: &'borrow mut dyn MeasurementControl) -> Self {
        Self {
            backend: Backend::Direct { iterations, control },
            runs: 0,
            failure: None,
        }
    }

    pub(crate) const fn gungraun() -> Self {
        Self {
            backend: Backend::Gungraun,
            runs: 0,
            failure: None,
        }
    }

    /// Defers setup until the active backend requests an input.
    pub fn setup<'setup, S, Input>(&'setup mut self, setup: S) -> SetupBencher<'setup, 'borrow, 'measurement, S, Input>
    where
        S: FnMut() -> Input,
    {
        SetupBencher {
            bencher: self,
            setup,
            input: PhantomData,
        }
    }

    /// Runs a workload that requires no setup input.
    pub fn run<Routine, Output>(&mut self, mut routine: Routine)
    where
        Routine: FnMut() -> Output,
    {
        self.runs = self.runs.saturating_add(1);
        match &mut self.backend {
            Backend::Criterion(bencher) => {
                bencher.iter_custom(|iterations| {
                    let mut elapsed = Duration::ZERO;
                    for _ in 0..iterations {
                        let started = Instant::now();
                        let output = invoke_without_input(&mut routine);
                        elapsed += started.elapsed();
                        black_box(&output);
                        drop(output);
                    }
                    elapsed
                });
            }
            Backend::Allocations { operation, samples } => {
                for _ in 0..*samples {
                    let span = operation.measure_process().iterations(1);
                    let output = invoke_without_input(&mut routine);
                    drop(span);
                    black_box(&output);
                    drop(output);
                }
            }
            Backend::Direct { iterations, control } => {
                for _ in 0..*iterations {
                    if let Err(error) = control.start() {
                        self.failure = Some(error);
                        return;
                    }
                    let output = match catch_unwind(AssertUnwindSafe(|| invoke_without_input(&mut routine))) {
                        Ok(output) => output,
                        Err(payload) => {
                            let _ = catch_unwind(AssertUnwindSafe(|| control.stop()));
                            resume_unwind(payload);
                        }
                    };
                    let stop = catch_unwind(AssertUnwindSafe(|| control.stop()));
                    black_box(&output);
                    drop(output);
                    match stop {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => {
                            self.failure = Some(error);
                            return;
                        }
                        Err(payload) => resume_unwind(payload),
                    }
                }
            }
            Backend::Gungraun => {
                let output = invoke_without_input(&mut routine);
                black_box(&output);
                drop(output);
            }
        }
    }

    pub(crate) fn finish(self, benchmark: &str) -> Result<(), crate::Error> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        if self.runs == 1 {
            Ok(())
        } else {
            Err(crate::Error::InvalidRunCount {
                benchmark: benchmark.to_owned(),
                count: self.runs,
            })
        }
    }
}

impl std::fmt::Debug for Bencher<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Bencher { .. }")
    }
}

/// A benchmark driver with deferred input setup.
pub struct SetupBencher<'setup, 'borrow, 'measurement, Setup, Input> {
    bencher: &'setup mut Bencher<'borrow, 'measurement>,
    setup: Setup,
    input: PhantomData<fn() -> Input>,
}

impl<Setup, Input> std::fmt::Debug for SetupBencher<'_, '_, '_, Setup, Input> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SetupBencher { .. }")
    }
}

impl<'setup, 'borrow, 'measurement, Setup, Input> SetupBencher<'setup, 'borrow, 'measurement, Setup, Input>
where
    Setup: FnMut() -> Input,
{
    /// Attaches explicit cleanup to be run on each workload output outside
    /// the measured region.
    pub fn cleanup<Cleanup>(self, cleanup: Cleanup) -> CleanupBencher<'setup, 'borrow, 'measurement, Setup, Input, Cleanup> {
        CleanupBencher { setup: self, cleanup }
    }

    /// Runs the workload with inputs produced outside the measured region.
    pub fn run<Routine, Output>(self, routine: Routine)
    where
        Routine: FnMut(Input) -> Output,
    {
        run_with_cleanup(self, routine, drop::<Output>);
    }
}

/// A benchmark driver with deferred setup and explicit output cleanup.
pub struct CleanupBencher<'setup, 'borrow, 'measurement, Setup, Input, Cleanup> {
    setup: SetupBencher<'setup, 'borrow, 'measurement, Setup, Input>,
    cleanup: Cleanup,
}

impl<Setup, Input, Cleanup> std::fmt::Debug for CleanupBencher<'_, '_, '_, Setup, Input, Cleanup> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CleanupBencher { .. }")
    }
}

impl<Setup, Input, Cleanup> CleanupBencher<'_, '_, '_, Setup, Input, Cleanup>
where
    Setup: FnMut() -> Input,
{
    /// Runs the workload, then passes its output to the cleanup callback
    /// outside the measured region.
    pub fn run<Routine, Output>(self, routine: Routine)
    where
        Routine: FnMut(Input) -> Output,
        Cleanup: FnMut(Output),
    {
        run_with_cleanup(self.setup, routine, self.cleanup);
    }
}

fn run_with_cleanup<Setup, Input, Routine, Output, Cleanup>(
    setup_bencher: SetupBencher<'_, '_, '_, Setup, Input>,
    mut routine: Routine,
    mut cleanup: Cleanup,
) where
    Setup: FnMut() -> Input,
    Routine: FnMut(Input) -> Output,
    Cleanup: FnMut(Output),
{
    let SetupBencher { bencher, mut setup, .. } = setup_bencher;

    bencher.runs = bencher.runs.saturating_add(1);
    match &mut bencher.backend {
        Backend::Criterion(criterion) => {
            criterion.iter_custom(|iterations| {
                let mut elapsed = Duration::ZERO;
                for _ in 0..iterations {
                    let input = black_box(setup());
                    let started = Instant::now();
                    let output = invoke_with_input(&mut routine, input);
                    elapsed += started.elapsed();
                    black_box(&output);
                    cleanup(output);
                }
                elapsed
            });
        }
        Backend::Allocations { operation, samples } => {
            for _ in 0..*samples {
                let input = black_box(setup());
                let span = operation.measure_process().iterations(1);
                let output = invoke_with_input(&mut routine, input);
                drop(span);
                black_box(&output);
                cleanup(output);
            }
        }
        Backend::Direct { iterations, control } => {
            for _ in 0..*iterations {
                let input = black_box(setup());
                if let Err(error) = control.start() {
                    bencher.failure = Some(error);
                    return;
                }
                let output = match catch_unwind(AssertUnwindSafe(|| invoke_with_input(&mut routine, input))) {
                    Ok(output) => output,
                    Err(payload) => {
                        let _ = catch_unwind(AssertUnwindSafe(|| control.stop()));
                        resume_unwind(payload);
                    }
                };
                let stop = catch_unwind(AssertUnwindSafe(|| control.stop()));
                black_box(&output);
                cleanup(output);
                match stop {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        bencher.failure = Some(error);
                        return;
                    }
                    Err(payload) => resume_unwind(payload),
                }
            }
        }
        Backend::Gungraun => {
            let input = black_box(setup());
            let output = invoke_with_input(&mut routine, input);
            black_box(&output);
            cleanup(output);
        }
    }
}

#[inline(never)]
fn invoke_without_input<Routine, Output>(routine: &mut Routine) -> Output
where
    Routine: FnMut() -> Output,
{
    routine()
}

#[inline(never)]
fn invoke_with_input<Routine, Input, Output>(routine: &mut Routine, input: Input) -> Output
where
    Routine: FnMut(Input) -> Output,
{
    routine(input)
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    #![allow(clippy::panic, reason = "panic lifecycle behavior is under test")]

    use std::cell::{Cell, RefCell};
    use std::io;
    use std::rc::Rc;

    use alloc_tracker::Session;
    use criterion::Criterion;

    use super::*;

    #[derive(Clone, Copy)]
    enum ControlFailure {
        Never,
        Start,
        Stop,
        PanicOnStop,
    }

    struct RecordingControl {
        events: Rc<RefCell<Vec<&'static str>>>,
        active: bool,
        failure: ControlFailure,
    }

    struct RecordedDrop {
        count: Rc<Cell<u64>>,
    }

    impl Drop for RecordedDrop {
        fn drop(&mut self) {
            self.count.set(self.count.get() + 1);
        }
    }

    impl RecordingControl {
        fn new(events: Rc<RefCell<Vec<&'static str>>>) -> Self {
            Self {
                events,
                active: false,
                failure: ControlFailure::Never,
            }
        }

        fn failing(events: Rc<RefCell<Vec<&'static str>>>, failure: ControlFailure) -> Self {
            Self {
                events,
                active: false,
                failure,
            }
        }
    }

    impl MeasurementControl for RecordingControl {
        fn start(&mut self) -> Result<(), crate::Error> {
            self.events.borrow_mut().push("start");
            assert!(!self.active, "measurement started twice");
            if matches!(self.failure, ControlFailure::Start) {
                return Err(control_error("start"));
            }
            self.active = true;
            Ok(())
        }

        fn stop(&mut self) -> Result<(), crate::Error> {
            self.events.borrow_mut().push("stop");
            assert!(self.active, "inactive measurement stopped");
            self.active = false;
            if matches!(self.failure, ControlFailure::PanicOnStop) {
                panic!("stop panic");
            }
            if matches!(self.failure, ControlFailure::Stop) {
                return Err(control_error("stop"));
            }
            Ok(())
        }
    }

    fn control_error(operation: &'static str) -> crate::Error {
        crate::Error::PerfControl(io::Error::other(operation))
    }

    fn assert_invalid_run_count(result: Result<(), crate::Error>, expected: u8) {
        assert!(matches!(
            result,
            Err(crate::Error::InvalidRunCount {
                benchmark,
                count
            }) if benchmark == "group/benchmark" && count == expected
        ));
    }

    #[test]
    fn direct_plain_run_repeats_complete_measurement_cycles() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut control = RecordingControl::new(Rc::clone(&events));
        let mut bencher = Bencher::direct(2, &mut control);

        bencher.run({
            let events = Rc::clone(&events);
            move || events.borrow_mut().push("run")
        });

        bencher.finish("group/benchmark").expect("one workload was registered");
        assert_eq!(*events.borrow(), ["start", "run", "stop", "start", "run", "stop"]);
    }

    #[test]
    fn direct_setup_and_cleanup_stay_outside_measurement() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut control = RecordingControl::new(Rc::clone(&events));
        let mut bencher = Bencher::direct(1, &mut control);

        bencher
            .setup({
                let events = Rc::clone(&events);
                move || {
                    events.borrow_mut().push("setup");
                    1_u8
                }
            })
            .cleanup({
                let events = Rc::clone(&events);
                move |_output| events.borrow_mut().push("cleanup")
            })
            .run({
                let events = Rc::clone(&events);
                move |input| {
                    events.borrow_mut().push("run");
                    input + 1
                }
            });

        bencher.finish("group/benchmark").expect("one workload was registered");
        assert_eq!(*events.borrow(), ["setup", "start", "run", "stop", "cleanup"]);
    }

    #[test]
    fn direct_default_cleanup_runs_after_each_stop() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let drops = Rc::new(Cell::new(0));
        let mut control = RecordingControl::new(Rc::clone(&events));
        let mut bencher = Bencher::direct(2, &mut control);

        bencher.setup(|| ()).run({
            let drops = Rc::clone(&drops);
            move |()| RecordedDrop { count: Rc::clone(&drops) }
        });

        bencher.finish("group/benchmark").expect("default cleanup workload");
        assert_eq!(drops.get(), 2);
        assert_eq!(*events.borrow(), ["start", "stop", "start", "stop"]);
    }

    #[test]
    fn direct_start_failure_skips_routine_stop_and_cleanup() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut control = RecordingControl::failing(Rc::clone(&events), ControlFailure::Start);
        let mut bencher = Bencher::direct(1, &mut control);

        bencher
            .setup({
                let events = Rc::clone(&events);
                move || {
                    events.borrow_mut().push("setup");
                }
            })
            .cleanup({
                let events = Rc::clone(&events);
                move |()| events.borrow_mut().push("cleanup")
            })
            .run({
                let events = Rc::clone(&events);
                move |()| events.borrow_mut().push("run")
            });

        assert!(matches!(
            bencher.finish("group/benchmark"),
            Err(crate::Error::PerfControl(error)) if error.to_string() == "start"
        ));
        assert_eq!(*events.borrow(), ["setup", "start"]);
    }

    #[test]
    fn direct_stop_failure_cleans_output_before_error_is_reported() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut control = RecordingControl::failing(Rc::clone(&events), ControlFailure::Stop);
        let mut bencher = Bencher::direct(2, &mut control);

        bencher
            .setup(|| ())
            .cleanup({
                let events = Rc::clone(&events);
                move |()| events.borrow_mut().push("cleanup")
            })
            .run({
                let events = Rc::clone(&events);
                move |()| events.borrow_mut().push("run")
            });

        assert!(matches!(
            bencher.finish("group/benchmark"),
            Err(crate::Error::PerfControl(error)) if error.to_string() == "stop"
        ));
        assert_eq!(*events.borrow(), ["start", "run", "stop", "cleanup"]);
    }

    #[test]
    fn direct_routine_panic_stops_measurement_and_preserves_panic() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut control = RecordingControl::new(Rc::clone(&events));
        let mut bencher = Bencher::direct(1, &mut control);

        let panic = catch_unwind(AssertUnwindSafe(|| {
            bencher.run(|| -> () { panic!("routine panic") });
        }));

        assert!(panic.is_err());
        assert_eq!(*events.borrow(), ["start", "stop"]);
    }

    #[test]
    fn direct_stop_panic_runs_cleanup_before_resuming_unwind() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut control = RecordingControl::failing(Rc::clone(&events), ControlFailure::PanicOnStop);
        let mut bencher = Bencher::direct(1, &mut control);

        let panic = catch_unwind(AssertUnwindSafe(|| {
            bencher
                .setup(|| ())
                .cleanup({
                    let events = Rc::clone(&events);
                    move |()| events.borrow_mut().push("cleanup")
                })
                .run(|()| ());
        }));

        assert!(panic.is_err());
        assert_eq!(*events.borrow(), ["start", "stop", "cleanup"]);
    }

    #[test]
    fn direct_setup_panic_never_starts_measurement() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut control = RecordingControl::new(Rc::clone(&events));
        let mut bencher = Bencher::direct(1, &mut control);

        let panic = catch_unwind(AssertUnwindSafe(|| {
            bencher.setup(|| -> () { panic!("setup panic") }).run(|()| ());
        }));

        assert!(panic.is_err());
        assert!(events.borrow().is_empty());
    }

    #[test]
    fn gungraun_supports_all_workload_forms() {
        let events = Rc::new(RefCell::new(Vec::new()));

        let mut plain = Bencher::gungraun();
        plain.run({
            let events = Rc::clone(&events);
            move || events.borrow_mut().push("plain")
        });
        plain.finish("group/benchmark").expect("plain workload");

        let mut default_cleanup = Bencher::gungraun();
        default_cleanup
            .setup({
                let events = Rc::clone(&events);
                move || {
                    events.borrow_mut().push("setup-default");
                }
            })
            .run({
                let events = Rc::clone(&events);
                move |()| events.borrow_mut().push("run-default")
            });
        default_cleanup.finish("group/benchmark").expect("setup workload");

        let mut explicit_cleanup = Bencher::gungraun();
        explicit_cleanup
            .setup({
                let events = Rc::clone(&events);
                move || {
                    events.borrow_mut().push("setup-explicit");
                }
            })
            .cleanup({
                let events = Rc::clone(&events);
                move |()| events.borrow_mut().push("cleanup")
            })
            .run({
                let events = Rc::clone(&events);
                move |()| events.borrow_mut().push("run-explicit")
            });
        explicit_cleanup.finish("group/benchmark").expect("cleanup workload");

        assert_eq!(
            *events.borrow(),
            ["plain", "setup-default", "run-default", "setup-explicit", "run-explicit", "cleanup"]
        );
    }

    #[test]
    fn allocations_support_all_workload_forms_and_repeat_cleanup_in_order() {
        let session = Session::new().no_stdout().no_file();
        let events = Rc::new(RefCell::new(Vec::new()));
        let plain_operation = session.operation("bencher-plain-test");
        let mut plain = Bencher::allocations(&plain_operation, 1);
        plain.run({
            let events = Rc::clone(&events);
            move || events.borrow_mut().push("plain")
        });
        plain.finish("group/benchmark").expect("plain workload");

        let default_operation = session.operation("bencher-default-cleanup-test");
        let drops = Rc::new(Cell::new(0));
        let mut default_cleanup = Bencher::allocations(&default_operation, 1);
        default_cleanup
            .setup({
                let events = Rc::clone(&events);
                move || {
                    events.borrow_mut().push("setup-default");
                }
            })
            .run({
                let events = Rc::clone(&events);
                let drops = Rc::clone(&drops);
                move |()| {
                    events.borrow_mut().push("run-default");
                    RecordedDrop { count: Rc::clone(&drops) }
                }
            });
        default_cleanup.finish("group/benchmark").expect("default cleanup workload");

        let explicit_operation = session.operation("bencher-explicit-cleanup-test");
        let mut explicit_cleanup = Bencher::allocations(&explicit_operation, 2);
        explicit_cleanup
            .setup({
                let events = Rc::clone(&events);
                move || {
                    events.borrow_mut().push("setup-explicit");
                }
            })
            .cleanup({
                let events = Rc::clone(&events);
                move |()| events.borrow_mut().push("cleanup")
            })
            .run({
                let events = Rc::clone(&events);
                move |()| events.borrow_mut().push("run-explicit")
            });

        explicit_cleanup.finish("group/benchmark").expect("allocation workload");
        assert_eq!(drops.get(), 1);
        assert_eq!(
            *events.borrow(),
            [
                "plain",
                "setup-default",
                "run-default",
                "setup-explicit",
                "run-explicit",
                "cleanup",
                "setup-explicit",
                "run-explicit",
                "cleanup"
            ]
        );
    }

    #[test]
    fn allocation_and_gungraun_setup_panics_skip_routine_and_cleanup() {
        let runs = Rc::new(Cell::new(0));
        let cleanups = Rc::new(Cell::new(0));
        let session = Session::new().no_stdout().no_file();
        let operation = session.operation("bencher-setup-panic-test");
        let mut allocation = Bencher::allocations(&operation, 2);

        let allocation_panic = catch_unwind(AssertUnwindSafe(|| {
            allocation
                .setup(|| -> () { panic!("allocation setup panic") })
                .cleanup({
                    let cleanups = Rc::clone(&cleanups);
                    move |()| cleanups.set(cleanups.get() + 1)
                })
                .run({
                    let runs = Rc::clone(&runs);
                    move |()| runs.set(runs.get() + 1)
                });
        }));
        assert!(allocation_panic.is_err());

        let mut gungraun = Bencher::gungraun();
        let gungraun_panic = catch_unwind(AssertUnwindSafe(|| {
            gungraun
                .setup(|| -> () { panic!("gungraun setup panic") })
                .cleanup({
                    let cleanups = Rc::clone(&cleanups);
                    move |()| cleanups.set(cleanups.get() + 1)
                })
                .run({
                    let runs = Rc::clone(&runs);
                    move |()| runs.set(runs.get() + 1)
                });
        }));

        assert!(gungraun_panic.is_err());
        assert_eq!(runs.get(), 0);
        assert_eq!(cleanups.get(), 0);
    }

    #[test]
    fn criterion_supports_all_workload_forms_and_cleans_each_output() {
        let plain_runs = Rc::new(Cell::new(0_u64));
        let default_runs = Rc::new(Cell::new(0_u64));
        let default_drops = Rc::new(Cell::new(0_u64));
        let setups = Rc::new(Cell::new(0_u64));
        let runs = Rc::new(Cell::new(0_u64));
        let cleanups = Rc::new(Cell::new(0_u64));
        let mut criterion = Criterion::default()
            .without_plots()
            .sample_size(10)
            .warm_up_time(std::time::Duration::from_millis(1))
            .measurement_time(std::time::Duration::from_millis(1))
            .nresamples(10);

        criterion.bench_function("bencher-plain", |criterion_bencher| {
            let mut bencher = Bencher::criterion(criterion_bencher);
            bencher.run({
                let plain_runs = Rc::clone(&plain_runs);
                move || plain_runs.set(plain_runs.get() + 1)
            });
            bencher.finish("group/benchmark").expect("plain criterion workload");
        });
        criterion.bench_function("bencher-default-cleanup", |criterion_bencher| {
            let mut bencher = Bencher::criterion(criterion_bencher);
            bencher.setup(|| ()).run({
                let default_runs = Rc::clone(&default_runs);
                let default_drops = Rc::clone(&default_drops);
                move |()| {
                    default_runs.set(default_runs.get() + 1);
                    RecordedDrop {
                        count: Rc::clone(&default_drops),
                    }
                }
            });
            bencher.finish("group/benchmark").expect("default cleanup criterion workload");
        });
        criterion.bench_function("bencher-explicit-cleanup", |criterion_bencher| {
            let mut bencher = Bencher::criterion(criterion_bencher);
            bencher
                .setup({
                    let setups = Rc::clone(&setups);
                    move || {
                        setups.set(setups.get() + 1);
                    }
                })
                .cleanup({
                    let cleanups = Rc::clone(&cleanups);
                    move |()| cleanups.set(cleanups.get() + 1)
                })
                .run({
                    let runs = Rc::clone(&runs);
                    move |()| runs.set(runs.get() + 1)
                });
            bencher.finish("group/benchmark").expect("criterion workload");
        });

        assert!(plain_runs.get() > 0);
        assert_eq!(default_drops.get(), default_runs.get());
        assert_eq!(setups.get(), runs.get());
        assert_eq!(cleanups.get(), runs.get());
    }

    #[test]
    fn criterion_setup_panic_skips_routine_and_cleanup() {
        let runs = Rc::new(Cell::new(0));
        let cleanups = Rc::new(Cell::new(0));
        let mut criterion = Criterion::default()
            .without_plots()
            .sample_size(10)
            .warm_up_time(std::time::Duration::from_millis(1))
            .measurement_time(std::time::Duration::from_millis(1))
            .nresamples(10);

        let panic = catch_unwind(AssertUnwindSafe(|| {
            criterion.bench_function("bencher-setup-panic", |criterion_bencher| {
                let mut bencher = Bencher::criterion(criterion_bencher);
                bencher
                    .setup(|| -> () { panic!("criterion setup panic") })
                    .cleanup({
                        let cleanups = Rc::clone(&cleanups);
                        move |()| cleanups.set(cleanups.get() + 1)
                    })
                    .run({
                        let runs = Rc::clone(&runs);
                        move |()| runs.set(runs.get() + 1)
                    });
            });
        }));

        assert!(panic.is_err());
        assert_eq!(runs.get(), 0);
        assert_eq!(cleanups.get(), 0);
    }

    #[test]
    fn zero_and_duplicate_workloads_are_rejected() {
        assert_invalid_run_count(Bencher::gungraun().finish("group/benchmark"), 0);

        let mut bencher = Bencher::gungraun();
        bencher.run(|| ());
        bencher.run(|| ());
        assert_invalid_run_count(bencher.finish("group/benchmark"), 2);
    }

    #[test]
    fn cleanup_panic_occurs_at_explicit_call_site() {
        let mut bencher = Bencher::gungraun();
        let panic = catch_unwind(AssertUnwindSafe(|| {
            bencher.setup(|| ()).cleanup(|()| panic!("cleanup panic")).run(|()| ());
        }));

        assert!(panic.is_err());
    }

    #[test]
    fn routine_panic_does_not_call_cleanup_during_unwind() {
        let calls = Rc::new(Cell::new(0));
        let mut bencher = Bencher::gungraun();
        let panic = catch_unwind(AssertUnwindSafe({
            let calls = Rc::clone(&calls);
            move || {
                bencher
                    .setup(|| ())
                    .cleanup(move |()| calls.set(calls.get() + 1))
                    .run(|()| -> () { panic!("routine panic") });
            }
        }));

        assert!(panic.is_err());
        assert_eq!(calls.get(), 0);
    }
}
