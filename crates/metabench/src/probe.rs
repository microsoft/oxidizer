// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::Path;
use std::{env, panic};

use gungraun::__internal::{InternalBenchRunMode, InternalLibFunctionKind, InternalMacroLibBench};
use serde::{Deserialize, Serialize};

use crate::BenchmarkIdentity;
use crate::probe_error::{
    DuplicateProbeBackendIdentity, DuplicateProbeIdentity, InvalidProbeArguments, MissingProbeDiscoveryPath, ProbeCaseLimit, ProbeError,
    ProbeIndexOutOfBounds, ProbeSelectionMismatch,
};

pub(crate) const DISCOVERY_ENV: &str = "METABENCH_INTERNAL_PROBE_DISCOVERY";
// Conservative headroom for large suites while bounding discovery records and worker fan-out.
pub(crate) const MAX_PROBE_CASES: usize = 10_000;

#[doc(hidden)]
#[derive(Debug)]
pub struct ProbeBenchmark {
    pub identity: BenchmarkIdentity,
    pub cases: &'static [InternalMacroLibBench],
}

#[doc(hidden)]
#[derive(Debug)]
pub struct ProbeGroup {
    pub benchmarks: &'static [ProbeBenchmark],
    pub setup: fn(bool) -> bool,
    pub teardown: fn(bool) -> bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct ProbeTarget {
    pub(crate) identity: String,
    pub(crate) native_identity: String,
    group: usize,
    benchmark: usize,
    case: usize,
    iteration: Option<usize>,
}

impl ProbeTarget {
    pub(crate) fn arguments(&self) -> Vec<OsString> {
        vec![
            "--metabench-probe".into(),
            self.group.to_string().into(),
            self.benchmark.to_string().into(),
            self.case.to_string().into(),
            self.iteration.map_or_else(|| "-".to_owned(), |index| index.to_string()).into(),
        ]
    }
}

fn case_identity(mut identity: String, case: &InternalMacroLibBench, iteration: Option<usize>) -> String {
    if let Some(id) = case.id_display {
        identity.push('/');
        identity.push_str(id);
    }
    if let Some(index) = iteration {
        identity.push('_');
        identity.push_str(&index.to_string());
    }
    identity
}

fn with_lifecycle(
    setup: fn(bool) -> bool,
    teardown: fn(bool) -> bool,
    body: impl FnOnce() -> Result<(), ProbeError>,
) -> Result<(), ProbeError> {
    setup(true);
    let result = panic::catch_unwind(panic::AssertUnwindSafe(body));
    teardown(true);
    match result {
        Ok(result) => result,
        Err(payload) => panic::resume_unwind(payload),
    }
}

fn execute_case(case: &InternalMacroLibBench, iteration: Option<usize>) -> Result<(), ProbeError> {
    match (case.func, iteration) {
        (InternalLibFunctionKind::Default(run), None) => {
            run(InternalBenchRunMode::Default);
            Ok(())
        }
        (InternalLibFunctionKind::Iter(run), Some(index)) => {
            let count = run(InternalBenchRunMode::Default, None);
            if index >= count {
                return Err(ProbeIndexOutOfBounds::new("iterator", index, count).into());
            }
            run(InternalBenchRunMode::Default, Some(index));
            Ok(())
        }
        _ => Err(ProbeSelectionMismatch::new(matches!(case.func, InternalLibFunctionKind::Iter(_)), iteration).into()),
    }
}

fn discover_targets(groups: &[ProbeGroup]) -> Result<Vec<ProbeTarget>, ProbeError> {
    let mut targets = Vec::new();
    let mut identities = BTreeSet::new();
    let mut native_identities = BTreeSet::new();
    for (group_index, group) in groups.iter().enumerate() {
        with_lifecycle(group.setup, group.teardown, || {
            for (benchmark_index, benchmark) in group.benchmarks.iter().enumerate() {
                for (case_index, case) in benchmark.cases.iter().enumerate() {
                    let (count, iterator) = match case.func {
                        InternalLibFunctionKind::Default(_) => (1, false),
                        InternalLibFunctionKind::Iter(count) => {
                            let count = count(InternalBenchRunMode::Default, None);
                            if count > MAX_PROBE_CASES.saturating_sub(targets.len()) {
                                return Err(ProbeCaseLimit::new(MAX_PROBE_CASES).into());
                            }
                            (count, true)
                        }
                    };
                    for index in 0..count {
                        let iteration = iterator.then_some(index);
                        if targets.len() == MAX_PROBE_CASES {
                            return Err(ProbeCaseLimit::new(MAX_PROBE_CASES).into());
                        }
                        let identity = case_identity(benchmark.identity.to_string(), case, iteration);
                        let native_identity = case_identity(benchmark.identity.gungraun_identity(), case, iteration);
                        if !identities.insert(identity.clone()) {
                            return Err(DuplicateProbeIdentity::new(identity).into());
                        }
                        if !native_identities.insert(native_identity.clone()) {
                            return Err(DuplicateProbeBackendIdentity::new(native_identity).into());
                        }
                        targets.push(ProbeTarget {
                            native_identity,
                            identity,
                            group: group_index,
                            benchmark: benchmark_index,
                            case: case_index,
                            iteration,
                        });
                    }
                }
            }
            Ok(())
        })?;
    }
    Ok(targets)
}

fn publish_targets(output: Option<OsString>, targets: &[ProbeTarget]) -> Result<(), ProbeError> {
    let output = output.ok_or_else(MissingProbeDiscoveryPath::new)?;
    crate::artifact::write_json(Path::new(&output), &targets)?;
    Ok(())
}

/// Executes only generated probe adapters, reusing the pinned native case table.
///
/// # Errors
///
/// Returns an error for invalid worker arguments, indices, or discovery output.
#[doc(hidden)]
pub fn run_probe_groups(groups: &[ProbeGroup], suite_setup: fn(bool) -> bool, suite_teardown: fn(bool) -> bool) -> Result<(), String> {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    run_probe_groups_inner(groups, suite_setup, suite_teardown, &arguments).map_err(|error| error.to_string())
}

fn run_probe_groups_inner(
    groups: &[ProbeGroup],
    suite_setup: fn(bool) -> bool,
    suite_teardown: fn(bool) -> bool,
    arguments: &[String],
) -> Result<(), ProbeError> {
    let listing = arguments == ["--metabench-probe-list"];
    let selected = if listing {
        None
    } else {
        if arguments.len() != 5 || arguments[0] != "--metabench-probe" {
            return Err(InvalidProbeArguments::new().into());
        }
        let parse = |index: usize| arguments[index].parse::<usize>().map_err(ProbeError::from);
        Some((
            parse(1)?,
            parse(2)?,
            parse(3)?,
            if arguments[4] == "-" { None } else { Some(parse(4)?) },
        ))
    };
    with_lifecycle(suite_setup, suite_teardown, || {
        if let Some((group, benchmark, case, iteration)) = selected {
            let group = groups
                .get(group)
                .ok_or_else(|| ProbeIndexOutOfBounds::new("group", group, groups.len()))?;
            let benchmark = group
                .benchmarks
                .get(benchmark)
                .ok_or_else(|| ProbeIndexOutOfBounds::new("benchmark", benchmark, group.benchmarks.len()))?;
            let case = benchmark
                .cases
                .get(case)
                .ok_or_else(|| ProbeIndexOutOfBounds::new("case", case, benchmark.cases.len()))?;
            with_lifecycle(group.setup, group.teardown, || execute_case(case, iteration))
        } else {
            let targets = discover_targets(groups)?;
            publish_targets(env::var_os(DISCOVERY_ENV), &targets)
        }
    })
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::error::Error as _;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    static SETUP: AtomicUsize = AtomicUsize::new(0);
    static TEARDOWN: AtomicUsize = AtomicUsize::new(0);

    thread_local! {
        static ITERATION_COUNT: Cell<usize> = const { Cell::new(0) };
        static COUNT_CALLS: Cell<usize> = const { Cell::new(0) };
        static EXECUTIONS: RefCell<Vec<Option<usize>>> = const { RefCell::new(Vec::new()) };
    }

    fn default_case(mode: InternalBenchRunMode) {
        assert_eq!(mode, InternalBenchRunMode::Default);
        EXECUTIONS.with_borrow_mut(|calls| calls.push(None));
    }

    fn iterator_case(mode: InternalBenchRunMode, index: Option<usize>) -> usize {
        assert_eq!(mode, InternalBenchRunMode::Default);
        match index {
            None => COUNT_CALLS.set(COUNT_CALLS.get() + 1),
            Some(index) => EXECUTIONS.with_borrow_mut(|calls| calls.push(Some(index))),
        }
        ITERATION_COUNT.get()
    }

    const fn native_case(func: InternalLibFunctionKind) -> InternalMacroLibBench {
        InternalMacroLibBench {
            args_display: None,
            config: None,
            consts_display: None,
            func,
            id_display: Some("case"),
        }
    }

    fn reset_callbacks(count: usize) {
        ITERATION_COUNT.set(count);
        COUNT_CALLS.set(0);
        EXECUTIONS.with_borrow_mut(Vec::clear);
    }

    #[test]
    fn case_identity_distinguishes_explicit_default_from_absent_id() {
        let mut case = native_case(InternalLibFunctionKind::Default(default_case));
        case.id_display = None;
        assert_eq!(case_identity("group/workload".into(), &case, None), "group/workload");
        case.id_display = Some("default");
        assert_eq!(case_identity("group/workload".into(), &case, None), "group/workload/default");
        assert_eq!(case_identity("group/workload".into(), &case, Some(0)), "group/workload/default_0");
    }

    #[test]
    fn selection_checks_kind_and_iterator_bounds_before_execution() {
        for count in [0_usize, 1, 3] {
            for iteration in [None, Some(0), Some(count.saturating_sub(1)), Some(count)] {
                for iterator in [false, true] {
                    reset_callbacks(count);
                    let case = native_case(if iterator {
                        InternalLibFunctionKind::Iter(iterator_case)
                    } else {
                        InternalLibFunctionKind::Default(default_case)
                    });
                    let result = execute_case(&case, iteration);
                    let valid = if iterator {
                        iteration.is_some_and(|index| index < count)
                    } else {
                        iteration.is_none()
                    };
                    if valid {
                        result.unwrap();
                        EXECUTIONS.with_borrow(|calls| assert_eq!(calls, &[iteration]));
                    } else {
                        let error = result.unwrap_err();
                        if iterator && let Some(index) = iteration {
                            let source = error.source().unwrap().downcast_ref::<ProbeIndexOutOfBounds>().unwrap();
                            assert_eq!((source.level, source.index, source.count), ("iterator", index, count));
                        } else {
                            let source = error.source().unwrap().downcast_ref::<ProbeSelectionMismatch>().unwrap();
                            assert_eq!((source.iterator, source.iteration), (iterator, iteration));
                        }
                        EXECUTIONS.with_borrow(|calls| assert_eq!(calls, &[] as &[Option<usize>]));
                    }
                    assert_eq!(COUNT_CALLS.get(), usize::from(iterator && iteration.is_some()));
                }
            }
        }
    }

    #[test]
    fn discovery_enforces_exact_and_remaining_case_budgets_without_execution() {
        static DEFAULT: [InternalMacroLibBench; 1] = [native_case(InternalLibFunctionKind::Default(default_case))];
        static ITERATOR: [InternalMacroLibBench; 1] = [native_case(InternalLibFunctionKind::Iter(iterator_case))];
        static DEFAULT_BENCHMARKS: [ProbeBenchmark; 1] = [ProbeBenchmark {
            identity: BenchmarkIdentity::__new("group", "default", "native_group", "native_default", true),
            cases: &DEFAULT,
        }];
        static ITERATOR_BENCHMARKS: [ProbeBenchmark; 1] = [ProbeBenchmark {
            identity: BenchmarkIdentity::__new("group", "iterator", "native_group", "native_iterator", true),
            cases: &ITERATOR,
        }];
        let group = |benchmarks| ProbeGroup {
            benchmarks,
            setup: |enabled| enabled,
            teardown: |enabled| enabled,
        };
        for prefix in [0, 1] {
            for count in [0, 1, 9_999, 10_000, 10_001] {
                reset_callbacks(count);
                let mut groups = Vec::new();
                if prefix == 1 {
                    groups.push(group(&DEFAULT_BENCHMARKS));
                }
                groups.push(group(&ITERATOR_BENCHMARKS));
                let result = discover_targets(&groups);
                if prefix + count <= 10_000 {
                    let targets = result.unwrap();
                    assert_eq!(targets.len(), prefix + count);
                    for (index, target) in targets.iter().skip(prefix).enumerate() {
                        assert_eq!(
                            (target.group, target.benchmark, target.case, target.iteration),
                            (prefix, 0, 0, Some(index))
                        );
                        assert_eq!(target.identity, format!("group/iterator/case_{index}"));
                        assert_eq!(target.native_identity, format!("native_group/native_iterator/case_{index}"));
                    }
                } else {
                    let error = result.unwrap_err();
                    assert_eq!(error.source().unwrap().downcast_ref::<ProbeCaseLimit>().unwrap().limit, 10_000);
                }
                assert_eq!(COUNT_CALLS.get(), 1);
                EXECUTIONS.with_borrow(|calls| assert_eq!(calls, &[] as &[Option<usize>]));
            }
        }
        for count in [10_000, 10_001] {
            reset_callbacks(9_999);
            let mut groups = vec![group(&ITERATOR_BENCHMARKS)];
            groups.extend((9_999..count).map(|_| group(&DEFAULT_BENCHMARKS)));
            let result = discover_targets(&groups);
            if count == 10_000 {
                assert_eq!(result.unwrap().len(), count);
            } else {
                let error = result.unwrap_err();
                assert_eq!(error.source().unwrap().downcast_ref::<ProbeCaseLimit>().unwrap().limit, 10_000);
            }
            assert_eq!(COUNT_CALLS.get(), 1);
            EXECUTIONS.with_borrow(|calls| assert_eq!(calls, &[] as &[Option<usize>]));
        }
    }

    #[test]
    fn discovery_rejects_colliding_iterator_and_declared_case_identities() {
        const ITERATOR: InternalMacroLibBench = InternalMacroLibBench {
            id_display: Some("sizes"),
            ..native_case(InternalLibFunctionKind::Iter(iterator_case))
        };
        const DECLARED: InternalMacroLibBench = InternalMacroLibBench {
            id_display: Some("sizes_0"),
            ..native_case(InternalLibFunctionKind::Default(default_case))
        };
        static ITERATOR_FIRST: [ProbeBenchmark; 1] = [ProbeBenchmark {
            identity: BenchmarkIdentity::__new("group", "iterator", "native_group", "native_iterator", true),
            cases: &[ITERATOR, DECLARED],
        }];
        static DECLARED_FIRST: [ProbeBenchmark; 1] = [ProbeBenchmark {
            identity: BenchmarkIdentity::__new("group", "iterator", "native_group", "native_iterator", true),
            cases: &[DECLARED, ITERATOR],
        }];
        for benchmarks in [&ITERATOR_FIRST, &DECLARED_FIRST] {
            reset_callbacks(1);
            let groups = [ProbeGroup {
                benchmarks,
                setup: |enabled| enabled,
                teardown: |enabled| enabled,
            }];
            let error = discover_targets(&groups).unwrap_err();
            assert_eq!(
                error.source().unwrap().downcast_ref::<DuplicateProbeIdentity>().unwrap().identity,
                "group/iterator/sizes_0"
            );
            EXECUTIONS.with_borrow(|calls| assert!(calls.is_empty()));
        }
    }

    #[test]
    fn discovery_rejects_distinct_canonical_cases_with_a_shared_backend_identity() {
        static CASES: [InternalMacroLibBench; 1] = [native_case(InternalLibFunctionKind::Default(default_case))];
        static BENCHMARKS: [ProbeBenchmark; 2] = [
            ProbeBenchmark {
                identity: BenchmarkIdentity::__new("group", "first", "native_group", "same", true),
                cases: &CASES,
            },
            ProbeBenchmark {
                identity: BenchmarkIdentity::__new("group", "second", "native_group", "same", true),
                cases: &CASES,
            },
        ];
        let groups = [ProbeGroup {
            benchmarks: &BENCHMARKS,
            setup: |enabled| enabled,
            teardown: |enabled| enabled,
        }];
        let error = discover_targets(&groups).unwrap_err();
        assert_eq!(
            error
                .source()
                .unwrap()
                .downcast_ref::<DuplicateProbeBackendIdentity>()
                .unwrap()
                .identity,
            "native_group/same/case"
        );
    }

    fn setup(enabled: bool) -> bool {
        assert!(enabled);
        SETUP.fetch_add(1, Ordering::Relaxed);
        true
    }

    fn teardown(enabled: bool) -> bool {
        assert!(enabled);
        TEARDOWN.fetch_add(1, Ordering::Relaxed);
        true
    }

    #[test]
    fn lifecycle_tears_down_after_success_error_and_panic() {
        with_lifecycle(setup, teardown, || Ok(())).unwrap();
        let error = with_lifecycle(setup, teardown, || Err(InvalidProbeArguments::new().into())).unwrap_err();
        assert!(error.source().unwrap().is::<InvalidProbeArguments>());
        let panic = panic::catch_unwind(|| with_lifecycle(setup, teardown, || panic!("workload failed")));
        panic.unwrap_err();
        assert_eq!(SETUP.load(Ordering::Relaxed), 3);
        assert_eq!(TEARDOWN.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn indexed_arguments_round_trip_without_native_filters() {
        let target = ProbeTarget {
            identity: "group/benchmark/case_2".into(),
            native_identity: "native/case_2".into(),
            group: 1,
            benchmark: 3,
            case: 4,
            iteration: Some(2),
        };
        assert_eq!(target.arguments(), ["--metabench-probe", "1", "3", "4", "2"].map(OsString::from));
        let encoded = serde_json::to_vec(&target).unwrap();
        let decoded: ProbeTarget = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.arguments(), target.arguments());
        assert_eq!(decoded.native_identity, target.native_identity);
        assert_eq!(ProbeTarget { iteration: None, ..target }.arguments()[4], "-");
    }

    #[test]
    fn worker_selection_preserves_typed_argument_and_index_errors() {
        static CASES: [InternalMacroLibBench; 1] = [native_case(InternalLibFunctionKind::Default(default_case))];
        static BENCHMARKS: [ProbeBenchmark; 1] = [ProbeBenchmark {
            identity: BenchmarkIdentity::__new("group", "default", "native_group", "native_default", true),
            cases: &CASES,
        }];
        let groups = [ProbeGroup {
            benchmarks: &BENCHMARKS,
            setup: |enabled| enabled,
            teardown: |enabled| enabled,
        }];
        let run = |arguments: &[&str]| {
            run_probe_groups_inner(
                &groups,
                |enabled| enabled,
                |enabled| enabled,
                &arguments.iter().map(|argument| (*argument).to_owned()).collect::<Vec<_>>(),
            )
        };
        for arguments in [&[][..], &["--other"][..], &["--metabench-probe", "0"][..]] {
            let error = run(arguments).unwrap_err();
            assert!(error.source().unwrap().is::<InvalidProbeArguments>());
            assert_eq!(error.to_string(), "expected private native probe worker arguments");
        }
        let error = run(&["--metabench-probe", "invalid", "0", "0", "-"]).unwrap_err();
        assert!(error.source().unwrap().is::<std::num::ParseIntError>());
        for (arguments, level) in [
            (["--metabench-probe", "1", "0", "0", "-"], "group"),
            (["--metabench-probe", "0", "1", "0", "-"], "benchmark"),
            (["--metabench-probe", "0", "0", "1", "-"], "case"),
        ] {
            let error = run(&arguments).unwrap_err();
            let source = error.source().unwrap().downcast_ref::<ProbeIndexOutOfBounds>().unwrap();
            assert_eq!((source.level, source.index, source.count), (level, 1, 1));
            assert_eq!(error.to_string(), format!("probe {level} index is out of bounds"));
        }
    }

    #[test]
    fn probe_artifact_errors_retain_the_original_source_until_worker_rendering() {
        let missing = publish_targets(None, &[]).unwrap_err();
        assert!(missing.source().unwrap().is::<MissingProbeDiscoveryPath>());
        assert_eq!(missing.to_string(), "missing native probe discovery artifact path");
        let error = ProbeError::from(crate::error::Error::ArtifactIo {
            path: "discovery.json".into(),
            source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "write denied"),
        });
        let artifact = error.source().unwrap().downcast_ref::<crate::error::Error>().unwrap();
        assert!(matches!(
            artifact,
            crate::error::Error::ArtifactIo { path, source }
                if path == Path::new("discovery.json") && source.kind() == std::io::ErrorKind::PermissionDenied
        ));
        assert_eq!(error.to_string(), "failed to access artifact discovery.json: write denied");
    }
}
