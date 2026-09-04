// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::PathBuf;
use std::{env, fmt, fs};

use crate::arguments::WORKER_FILTERS_ENV;
use crate::artifact::ARTIFACT_DIR_ENV;
use crate::{Bencher, Engines, Error, Mode};

type BenchmarkFn = dyn for<'borrow, 'measurement> Fn(&mut Bencher<'borrow, 'measurement>);

pub(crate) struct Benchmark {
    pub(crate) name: String,
    pub(crate) case: Option<String>,
    pub(crate) engines: Engines,
    pub(crate) function: Box<BenchmarkFn>,
}

impl fmt::Debug for Benchmark {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Benchmark")
            .field("name", &self.name)
            .field("case", &self.case)
            .field("engines", &self.engines)
            .finish_non_exhaustive()
    }
}

/// A named category of related benchmark workloads.
///
/// Obtain a group through [`BenchmarkSuite::benchmark_group`] and add
/// workloads with [`BenchmarkGroup::benchmark`].
pub struct BenchmarkGroup {
    pub(crate) name: String,
    pub(crate) benchmarks: Vec<Benchmark>,
    selected_identity: Option<String>,
}

impl BenchmarkGroup {
    fn new(name: String, selected_identity: Option<String>) -> Self {
        Self {
            name,
            benchmarks: Vec::new(),
            selected_identity,
        }
    }

    /// Registers a named benchmark for the selected engines.
    pub fn benchmark<F>(&mut self, name: impl Into<String>, engines: Engines, benchmark: F)
    where
        F: for<'borrow, 'measurement> Fn(&mut Bencher<'borrow, 'measurement>) + 'static,
    {
        let name = name.into();
        if !self.accepts(&name, None) {
            return;
        }
        self.benchmarks.push(Benchmark {
            name,
            case: None,
            engines,
            function: Box::new(benchmark),
        });
    }

    /// Registers one generated data case.
    #[doc(hidden)]
    pub fn benchmark_case<F>(&mut self, name: impl Into<String>, case: Option<String>, engines: Engines, benchmark: F)
    where
        F: for<'borrow, 'measurement> Fn(&mut Bencher<'borrow, 'measurement>) + 'static,
    {
        let name = name.into();
        if !self.accepts(&name, case.as_deref()) {
            return;
        }
        self.benchmarks.push(Benchmark {
            name,
            case,
            engines,
            function: Box::new(benchmark),
        });
    }

    fn accepts(&self, name: &str, case: Option<&str>) -> bool {
        self.selected_identity.as_deref().is_none_or(|selected| {
            let expected = match case {
                Some(case) => format!("{}/{name}/{case}", self.name),
                None => format!("{}/{name}", self.name),
            };
            selected == expected
        })
    }
}

impl fmt::Debug for BenchmarkGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BenchmarkGroup")
            .field("name", &self.name)
            .field("benchmarks", &self.benchmarks)
            .field("selected_identity", &self.selected_identity)
            .finish()
    }
}

/// Registry containing every named benchmark group in one executable.
#[derive(Debug, Default)]
pub struct BenchmarkSuite {
    pub(crate) groups: Vec<BenchmarkGroup>,
    selected_identity: Option<String>,
}

impl BenchmarkSuite {
    /// Creates an empty benchmark suite.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            groups: Vec::new(),
            selected_identity: None,
        }
    }

    /// Returns the named group, creating it when first requested.
    ///
    /// # Panics
    ///
    /// Panics only if the group inserted immediately before returning cannot
    /// be retrieved, which would indicate an internal invariant violation.
    pub fn benchmark_group(&mut self, name: impl Into<String>) -> &mut BenchmarkGroup {
        let name = name.into();
        if let Some(index) = self.groups.iter().position(|group| group.name == name) {
            return &mut self.groups[index];
        }
        self.groups.push(BenchmarkGroup::new(name, self.selected_identity.clone()));
        self.groups.last_mut().expect("a benchmark group was just inserted")
    }

    pub(crate) fn register(register: fn(&mut Self)) -> Result<Self, Error> {
        Self::register_selected(register, None)
    }

    fn register_selected(register: fn(&mut Self), selected_identity: Option<&str>) -> Result<Self, Error> {
        let mut suite = Self {
            groups: Vec::new(),
            selected_identity: selected_identity.map(str::to_owned),
        };
        register(&mut suite);
        for group in &suite.groups {
            if group.name.is_empty() || group.name.contains('/') {
                return Err(Error::InvalidGroupName(group.name.clone()));
            }
            let mut names = HashSet::with_capacity(group.benchmarks.len());
            for benchmark in &group.benchmarks {
                if benchmark.name.is_empty() || benchmark.name.contains('/') {
                    return Err(Error::InvalidBenchmarkName {
                        group: group.name.clone(),
                        benchmark: benchmark.name.clone(),
                    });
                }
                if benchmark.engines.is_empty() {
                    return Err(Error::BenchmarkWithoutEngines(format!("{}/{}", group.name, benchmark.name)));
                }
                if benchmark.case.as_ref().is_some_and(|case| case.is_empty() || case.contains('/')) {
                    return Err(Error::InvalidCaseName {
                        benchmark: format!("{}/{}", group.name, benchmark.name),
                        case: benchmark.case.clone().unwrap_or_default(),
                    });
                }
                if !names.insert((benchmark.name.as_str(), benchmark.case.as_deref())) {
                    return Err(Error::DuplicateBenchmark(format!(
                        "{}/{}{}",
                        group.name,
                        benchmark.name,
                        benchmark.case.as_deref().map_or_else(String::new, |case| format!("/{case}"))
                    )));
                }
            }
        }
        Ok(suite)
    }

    pub(crate) fn register_worker(register: fn(&mut Self)) -> Result<Self, Error> {
        let mut suite = Self::register(register)?;
        let Some(filters) = env::var_os(WORKER_FILTERS_ENV) else {
            return Ok(suite);
        };
        let filters = parse_worker_filters(&filters)?;
        if !suite.retain_filters(&filters) {
            return Err(Error::NoBenchmarksMatched(filters));
        }
        Ok(suite)
    }

    pub(crate) fn benchmarks(&self) -> impl Iterator<Item = (&str, &Benchmark)> {
        self.groups
            .iter()
            .flat_map(|group| group.benchmarks.iter().map(move |benchmark| (group.name.as_str(), benchmark)))
    }

    pub(crate) fn contains_mode(&self, mode: Mode) -> bool {
        self.benchmarks().any(|(_, benchmark)| benchmark.engines.contains_mode(mode))
    }

    pub(crate) fn retain_mode(&mut self, mode: Mode) {
        for group in &mut self.groups {
            group.benchmarks.retain(|benchmark| benchmark.engines.contains_mode(mode));
        }
        self.groups.retain(|group| !group.benchmarks.is_empty());
    }

    pub(crate) fn retain_benchmark(&mut self, selected: &str) -> bool {
        for group in &mut self.groups {
            group.benchmarks.retain(|benchmark| benchmark.identity(&group.name) == selected);
        }

        self.groups.retain(|group| !group.benchmarks.is_empty());
        self.benchmarks().next().is_some()
    }

    pub(crate) fn retain_filters(&mut self, filters: &[String]) -> bool {
        if filters.is_empty() {
            return true;
        }
        for group in &mut self.groups {
            group.benchmarks.retain(|benchmark| {
                let identity = benchmark.identity(&group.name);
                filters.iter().any(|filter| wildcard_matches(filter, &identity))
            });
        }
        self.groups.retain(|group| !group.benchmarks.is_empty());
        self.benchmarks().next().is_some()
    }
}

fn parse_worker_filters(value: &OsStr) -> Result<Vec<String>, Error> {
    let encoded = value
        .to_str()
        .ok_or_else(|| Error::InvalidFilter(value.to_string_lossy().into_owned()))?;
    let filters = serde_json::from_str::<Vec<String>>(encoded).map_err(|error| Error::InvalidFilter(error.to_string()))?;
    if filters.iter().any(String::is_empty) {
        return Err(Error::InvalidFilter(String::new()));
    }
    Ok(filters)
}

impl Benchmark {
    pub(crate) fn identity(&self, group: &str) -> String {
        match &self.case {
            Some(case) => format!("{group}/{}/{case}", self.name),
            None => format!("{group}/{}", self.name),
        }
    }
}

fn wildcard_matches(pattern: &str, value: &str) -> bool {
    let value = value.chars().collect::<Vec<_>>();
    let mut matches = vec![false; value.len() + 1];
    matches[0] = true;
    for pattern_character in pattern.chars() {
        if pattern_character == '*' {
            for index in 1..matches.len() {
                matches[index] = matches[index] || matches[index - 1];
            }
            continue;
        }
        for index in (1..matches.len()).rev() {
            matches[index] = matches[index - 1] && (pattern_character == '?' || pattern_character == value[index - 1]);
        }
        matches[0] = false;
    }
    matches.last().copied().unwrap_or(false)
}

/// A dynamically registered workload selected by Gungraun.
#[doc(hidden)]
#[derive(Debug)]
pub struct GungraunCase {
    group: String,
    benchmark: Benchmark,
}

impl GungraunCase {
    /// Runs the selected workload once.
    ///
    /// # Errors
    ///
    /// Returns an error unless the benchmark callback defines exactly one
    /// workload invocation.
    pub fn run(self) -> Result<(), Error> {
        let mut bencher = Bencher::gungraun();
        (self.benchmark.function)(&mut bencher);
        bencher.finish(&self.benchmark.identity(&self.group))
    }
}

/// Returns the number of dynamically registered Gungraun cases.
#[doc(hidden)]
pub fn gungraun_registry_len(register: fn(&mut BenchmarkSuite)) -> Result<usize, Error> {
    let suite = BenchmarkSuite::register_worker(register)?;
    let identities = suite
        .benchmarks()
        .filter(|(_, benchmark)| benchmark.engines.contains(Engines::GUNGRAUN))
        .map(|(group, benchmark)| benchmark.identity(group))
        .collect::<Vec<_>>();
    if let Some(path) = gungraun_identity_path() {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| Error::ArtifactIo {
                path: parent.to_owned(),
                source,
            })?;
        }
        let contents = serde_json::to_vec(&identities).map_err(|source| Error::ArtifactJson {
            path: path.clone(),
            source,
        })?;
        fs::write(&path, contents).map_err(|source| Error::ArtifactIo { path, source })?;
    }
    Ok(identities.len())
}

/// Selects one Gungraun case through the controller's stable identity manifest.
///
/// # Errors
///
/// Returns an error when registration is invalid or the index is out of range.
#[doc(hidden)]
pub fn gungraun_registry_case(register: fn(&mut BenchmarkSuite), index: usize) -> Result<GungraunCase, Error> {
    if let Some(path) = gungraun_identity_path()
        && path.is_file()
    {
        let contents = fs::read(&path).map_err(|source| Error::ArtifactIo {
            path: path.clone(),
            source,
        })?;
        let identities = serde_json::from_slice::<Vec<String>>(&contents).map_err(|source| Error::ArtifactJson {
            path: path.clone(),
            source,
        })?;
        let identity = identities.get(index).ok_or(Error::InvalidGungraunCaseIndex(index))?;
        return select_gungraun_case(register, identity);
    }

    let suite = BenchmarkSuite::register_worker(register)?;
    let mut cases = suite.groups.into_iter().flat_map(|group| {
        let group_name = group.name;
        group
            .benchmarks
            .into_iter()
            .filter(|benchmark| benchmark.engines.contains(Engines::GUNGRAUN))
            .map(move |benchmark| (group_name.clone(), benchmark))
    });
    let (group, benchmark) = cases.nth(index).ok_or(Error::InvalidGungraunCaseIndex(index))?;
    Ok(GungraunCase { group, benchmark })
}

fn select_gungraun_case(register: fn(&mut BenchmarkSuite), identity: &str) -> Result<GungraunCase, Error> {
    let suite = BenchmarkSuite::register_selected(register, Some(identity))?;
    suite
        .groups
        .into_iter()
        .flat_map(|group| {
            let group_name = group.name;
            group.benchmarks.into_iter().map(move |benchmark| (group_name.clone(), benchmark))
        })
        .find(|(group, benchmark)| benchmark.identity(group) == identity)
        .map(|(group, benchmark)| GungraunCase { group, benchmark })
        .ok_or_else(|| Error::UnknownBenchmark(identity.to_owned()))
}

fn gungraun_identity_path() -> Option<PathBuf> {
    env::var_os(ARTIFACT_DIR_ENV)
        .map(PathBuf::from)
        .map(|root| root.join("gungraun").join("metabench-identities.json"))
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    fn duplicate_registration(suite: &mut BenchmarkSuite) {
        let group = suite.benchmark_group("group");
        group.benchmark("duplicate", Engines::ALL, |bencher| bencher.run(|| 1_u8));
        group.benchmark("duplicate", Engines::ALL, |bencher| bencher.run(|| 2_u8));
    }

    #[test]
    fn rejects_duplicate_benchmark_names_within_a_group() {
        assert!(matches!(
            BenchmarkSuite::register(duplicate_registration),
            Err(Error::DuplicateBenchmark(name)) if name == "group/duplicate"
        ));
    }

    fn two_groups(suite: &mut BenchmarkSuite) {
        suite
            .benchmark_group("first")
            .benchmark("same_name", Engines::CRITERION, |bencher| {
                bencher.run(|| 1_u8);
            });
        suite
            .benchmark_group("second")
            .benchmark("same_name", Engines::GUNGRAUN, |bencher| {
                bencher.run(|| 2_u8);
            });
    }

    #[test]
    fn gungraun_case_selection_preserves_filtered_registration_order() {
        let case = gungraun_registry_case(two_groups, 0).expect("valid case");
        assert_eq!(case.group, "second");
        assert_eq!(case.benchmark.name, "same_name");
    }

    #[test]
    fn gungraun_case_selection_reports_an_out_of_range_index() {
        let error = gungraun_registry_case(two_groups, 1).unwrap_err();

        assert!(matches!(error, Error::InvalidGungraunCaseIndex(1)));
        assert_eq!(error.to_string(), "Gungraun case index 1 is out of range");
    }

    fn empty_engines(suite: &mut BenchmarkSuite) {
        suite.benchmark_group("group").benchmark("empty", Engines::empty(), |bencher| {
            bencher.run(|| 1_u8);
        });
    }

    #[test]
    fn rejects_benchmarks_without_engines() {
        assert!(matches!(
            BenchmarkSuite::register(empty_engines),
            Err(Error::BenchmarkWithoutEngines(name)) if name == "group/empty"
        ));
    }

    #[test]
    fn filters_complete_benchmark_identities_with_globs() {
        let mut suite = BenchmarkSuite::register(two_groups).expect("valid suite");

        assert!(suite.retain_filters(&["second/*".to_owned()]));
        assert_eq!(
            suite
                .benchmarks()
                .map(|(group, benchmark)| benchmark.identity(group))
                .collect::<Vec<_>>(),
            ["second/same_name"]
        );
    }

    #[test]
    fn worker_filters_reject_empty_entries() {
        assert!(matches!(
            parse_worker_filters(OsStr::new(r#"["group/*", ""]"#)),
            Err(Error::InvalidFilter(filter)) if filter.is_empty()
        ));
        assert_eq!(
            parse_worker_filters(OsStr::new(r#"["group/*"]"#)).expect("valid worker filters"),
            ["group/*"]
        );
    }

    #[test]
    fn wildcard_matching_supports_star_and_question_mark() {
        assert!(wildcard_matches("hash_*/insert/?ize=10", "hash_map/insert/size=10"));
        assert!(!wildcard_matches("hash_*/insert/?ize=20", "hash_map/insert/size=10"));
    }

    fn invalid_group(suite: &mut BenchmarkSuite) {
        suite
            .benchmark_group("invalid/group")
            .benchmark("work", Engines::CRITERION, |bencher| {
                bencher.run(|| 1_u8);
            });
    }

    fn invalid_benchmark(suite: &mut BenchmarkSuite) {
        suite.benchmark_group("group").benchmark("", Engines::CRITERION, |bencher| {
            bencher.run(|| 1_u8);
        });
    }

    fn invalid_case(suite: &mut BenchmarkSuite) {
        suite
            .benchmark_group("group")
            .benchmark_case("work", Some("invalid/case".to_owned()), Engines::CRITERION, |bencher| {
                bencher.run(|| 1_u8);
            });
    }

    #[test]
    fn validates_every_identity_component() {
        assert!(matches!(
            BenchmarkSuite::register(invalid_group),
            Err(Error::InvalidGroupName(name)) if name == "invalid/group"
        ));
        assert!(matches!(
            BenchmarkSuite::register(invalid_benchmark),
            Err(Error::InvalidBenchmarkName { group, benchmark })
                if group == "group" && benchmark.is_empty()
        ));
        assert!(matches!(
            BenchmarkSuite::register(invalid_case),
            Err(Error::InvalidCaseName { benchmark, case })
                if benchmark == "group/work" && case == "invalid/case"
        ));
    }

    fn multiple_cases(suite: &mut BenchmarkSuite) {
        let group = suite.benchmark_group("group");
        for case in ["one", "two", "three"] {
            group.benchmark_case("work", Some(case.to_owned()), Engines::GUNGRAUN, |bencher| bencher.run(|| 1_u8));
        }
    }

    fn reversed_cases(suite: &mut BenchmarkSuite) {
        let group = suite.benchmark_group("group");
        for case in ["three", "two", "one"] {
            group.benchmark_case("work", Some(case.to_owned()), Engines::GUNGRAUN, |bencher| bencher.run(|| 1_u8));
        }
    }

    #[test]
    fn selected_registration_materializes_only_the_stable_identity() {
        let suite = BenchmarkSuite::register_selected(multiple_cases, Some("group/work/two")).expect("selected registration");
        let identities = suite
            .benchmarks()
            .map(|(group, benchmark)| benchmark.identity(group))
            .collect::<Vec<_>>();

        assert_eq!(identities, ["group/work/two"]);
    }

    #[test]
    fn stable_identity_selection_does_not_depend_on_registration_order() {
        for register in [multiple_cases as fn(&mut BenchmarkSuite), reversed_cases as fn(&mut BenchmarkSuite)] {
            let case = select_gungraun_case(register, "group/work/two").expect("stable identity");
            assert_eq!(case.group, "group");
            assert_eq!(case.benchmark.case.as_deref(), Some("two"));
        }
    }

    #[test]
    fn mode_and_exact_identity_filters_remove_empty_groups() {
        let mut suite = BenchmarkSuite::register(two_groups).expect("valid suite");
        suite.retain_mode(Mode::Callgrind);
        assert_eq!(suite.groups.len(), 1);
        assert!(suite.retain_benchmark("second/same_name"));
        assert!(!suite.retain_benchmark("missing/work"));
        assert!(suite.groups.is_empty());
    }

    #[test]
    fn wildcard_matching_is_unicode_scalar_aware() {
        assert!(wildcard_matches("gr?p/*/café", "grüp/work/café"));
        assert!(!wildcard_matches("gr??up/*", "grüp/work"));
    }

    #[test]
    fn bolero_wildcards_match_an_independent_oracle() {
        fn text(bytes: [u8; 8], pattern: bool) -> String {
            bytes
                .into_iter()
                .map(|byte| match (pattern, byte % 5) {
                    (true, 0) => '*',
                    (true, 1) => '?',
                    (_, 2) => 'é',
                    (_, 3) => 'b',
                    _ => 'a',
                })
                .collect()
        }

        fn reference(pattern: &[char], value: &[char]) -> bool {
            match pattern {
                [] => value.is_empty(),
                ['*', rest @ ..] => reference(rest, value) || (!value.is_empty() && reference(pattern, &value[1..])),
                ['?', rest @ ..] => !value.is_empty() && reference(rest, &value[1..]),
                [first, rest @ ..] => value.first() == Some(first) && reference(rest, &value[1..]),
            }
        }

        bolero::check!()
            .with_type::<([u8; 8], [u8; 8])>()
            .cloned()
            .for_each(|(pattern_bytes, value_bytes)| {
                let pattern = text(pattern_bytes, true);
                let value = text(value_bytes, false);
                let pattern_chars = pattern.chars().collect::<Vec<_>>();
                let value_chars = value.chars().collect::<Vec<_>>();
                assert_eq!(wildcard_matches(&pattern, &value), reference(&pattern_chars, &value_chars));
            });
    }
}
