// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(not(test))]
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{env, io};

use alloc_tracker::{ProcessSpan, Report, Session};
use serde::{Deserialize, Serialize};

#[cfg(test)]
use self::allocation_calls as allocator;
#[cfg(test)]
use self::allocation_calls as fs;
#[cfg(not(test))]
use crate::allocator;
use crate::error::Error;
use crate::report::{Metric, MetricDirection, NativeResult};

pub(crate) const ARTIFACT_ENV: &str = "METABENCH_INTERNAL_ALLOCATION_ARTIFACT";
pub(crate) const IDENTITY_ENV: &str = "METABENCH_INTERNAL_ALLOCATION_IDENTITY";

static ACTIVE: AtomicBool = AtomicBool::new(false);
static MEASURED: AtomicBool = AtomicBool::new(false);
static SESSION: LazyLock<Session> = LazyLock::new(|| Session::new().no_stdout().no_file());

#[derive(Debug, Deserialize, Serialize)]
struct AllocationArtifact {
    identity: String,
    allocated_bytes: u64,
    allocation_count: u64,
}

#[inline]
/// Begins measuring the first explicit workload boundary in an allocation worker.
#[doc(hidden)]
pub fn begin() -> Option<ProcessSpan> {
    (ACTIVE.load(Ordering::Relaxed) && !MEASURED.swap(true, Ordering::Relaxed)).then(|| {
        let operation = SESSION.operation("workload");
        operation.measure_process().iterations(1)
    })
}

/// Enables collection after validating an allocation worker's token and mode.
///
/// Enable tracking and initialize the session before the workload boundary so
/// session setup cannot contribute allocations to the selected workload.
#[cfg_attr(coverage_nightly, coverage(off))] // exercised in generated allocation-worker executables
pub(crate) fn prime() {
    // #[gamma::skip(call_result.default, reason = "prime_with returns unit and default_after preserves initialization and activation")]
    prime_with(allocation_environment, || {
        enable_tracking();
        initialize_session();
    });
}

fn prime_with(var_os: impl FnOnce(&str) -> Option<std::ffi::OsString>, initialize: impl FnOnce()) {
    if var_os(ARTIFACT_ENV).is_some() {
        initialize();
        ACTIVE.store(true, Ordering::Relaxed);
    }
}

pub(crate) fn write_worker_artifact() -> Result<(), Error> {
    write_worker_artifact_with(
        || allocation_environment(ARTIFACT_ENV),
        allocation_identity,
        || SESSION.to_report(),
        write_json,
    )
}

fn write_worker_artifact_with(
    artifact_path: impl FnOnce() -> Option<std::ffi::OsString>,
    identity: impl FnOnce() -> Result<String, env::VarError>,
    report: impl FnOnce() -> Report,
    write: impl FnOnce(&Path, &AllocationArtifact) -> Result<(), Error>,
) -> Result<(), Error> {
    let Some(path) = artifact_path().map(PathBuf::from) else {
        return Ok(());
    };
    let identity = identity().map_err(|error| Error::ArtifactFormat {
        path: path.clone(),
        message: format!("missing allocation benchmark identity: {error}"),
    })?;
    let report = report();
    let operation = report
        .operations()
        .find_map(|(name, operation)| (name == "workload").then_some(operation))
        .ok_or_else(|| Error::ArtifactFormat {
            path: path.clone(),
            message: format!("benchmark '{identity}' did not execute its generated allocation probe"),
        })?;
    if operation.total_iterations() != 1 {
        return Err(Error::ArtifactFormat {
            path,
            message: format!(
                "benchmark '{identity}' recorded {} allocation iterations instead of one",
                operation.total_iterations()
            ),
        });
    }
    write(
        &path,
        &AllocationArtifact {
            identity,
            allocated_bytes: operation.total_bytes_allocated(),
            allocation_count: operation.total_allocations_count(),
        },
    )
}

pub(crate) fn read_artifact(path: &Path, native_identity: String, relative_path: String) -> Result<NativeResult, Error> {
    read_artifact_with(path, native_identity, relative_path, crate::artifact::read_json)
}

fn read_artifact_with(
    path: &Path,
    native_identity: String,
    relative_path: String,
    read: impl FnOnce(&Path) -> Result<AllocationArtifact, Error>,
) -> Result<NativeResult, Error> {
    let artifact = read(path)?;
    Ok(NativeResult {
        identity: artifact.identity,
        native_identity,
        source: "alloc_tracker".to_owned(),
        // #[gamma::skip(collection.reverse_array, reason = "the two distinct keys are collected into a BTreeMap; insertion order does not affect the map or its sorted iteration")]
        metrics: [
            (
                "Allocated bytes".to_owned(),
                Metric::integer(
                    "alloc_tracker",
                    "Allocated bytes",
                    artifact.allocated_bytes,
                    MetricDirection::LowerIsBetter,
                ),
            ),
            (
                "Allocations".to_owned(),
                Metric::integer(
                    "alloc_tracker",
                    "Allocations",
                    artifact.allocation_count,
                    MetricDirection::LowerIsBetter,
                ),
            ),
        ]
        .into(),
        raw_artifacts: vec![relative_path],
    })
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), Error> {
    write_json_with(path, value, allocation_create_parent, allocation_write_bytes)
}

fn allocation_environment(name: &str) -> Option<std::ffi::OsString> {
    #[cfg(test)]
    if let Some(sources) = TEST_ALLOCATION_SOURCES.get() {
        return (sources.environment)(name);
    }
    env::var_os(name)
}

fn allocation_identity() -> Result<String, env::VarError> {
    #[cfg(test)]
    if let Some(sources) = TEST_ALLOCATION_SOURCES.get() {
        return (sources.identity)();
    }
    env::var(IDENTITY_ENV)
}

fn enable_tracking() {
    allocator::enable_tracking();
}

fn initialize_session() {
    #[cfg(test)]
    use allocation_calls::Session as LazyLock;
    LazyLock::force(&SESSION);
}

fn allocation_create_parent(path: &Path) -> io::Result<()> {
    #[cfg(test)]
    if let Some(sources) = TEST_ALLOCATION_SOURCES.get() {
        return (sources.create_parent)(path);
    }
    fs::create_dir_all(path)
}

fn allocation_write_bytes(path: &Path, bytes: Vec<u8>) -> io::Result<()> {
    fs::write(path, bytes)
}

#[cfg(test)]
mod allocation_calls {
    use super::*;

    pub(super) struct Session;

    impl Session {
        pub(super) fn force(session: &LazyLock<alloc_tracker::Session>) {
            assert!(std::ptr::eq(session, &raw const SESSION));
            if let Some(sources) = TEST_ALLOCATION_SOURCES.get() {
                (sources.session)();
            } else {
                LazyLock::force(session);
            }
        }
    }

    pub(super) fn enable_tracking() {
        if let Some(sources) = TEST_ALLOCATION_SOURCES.get() {
            (sources.track)();
        } else {
            crate::allocator::enable_tracking();
        }
    }

    pub(super) fn create_dir_all(path: &Path) -> io::Result<()> {
        if let Some(sources) = TEST_ALLOCATION_SOURCES.get() {
            (sources.create_parent)(path)
        } else {
            std::fs::create_dir_all(path)
        }
    }

    pub(super) fn write(path: &Path, bytes: Vec<u8>) -> io::Result<()> {
        if let Some(sources) = TEST_ALLOCATION_SOURCES.get() {
            (sources.write)(path, bytes)
        } else {
            std::fs::write(path, bytes)
        }
    }
}

#[cfg(test)]
#[derive(Clone, Copy)]
struct TestAllocationSources {
    environment: fn(&str) -> Option<std::ffi::OsString>,
    identity: fn() -> Result<String, env::VarError>,
    track: fn(),
    session: fn(),
    create_parent: fn(&Path) -> io::Result<()>,
    write: fn(&Path, Vec<u8>) -> io::Result<()>,
}

#[cfg(test)]
thread_local! {
    static TEST_ALLOCATION_SOURCES: std::cell::Cell<Option<TestAllocationSources>> = const { std::cell::Cell::new(None) };
}

fn write_json_with(
    path: &Path,
    value: &impl Serialize,
    create_parent: impl FnOnce(&Path) -> io::Result<()>,
    write: impl FnOnce(&Path, Vec<u8>) -> io::Result<()>,
) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        create_parent(parent).map_err(|source| Error::ArtifactIo {
            path: parent.to_owned(),
            source,
        })?;
    }
    let bytes = serde_json::to_vec_pretty(value).map_err(|source| Error::ArtifactJson {
        path: path.to_owned(),
        source,
    })?;
    write(path, bytes).map_err(|source| Error::ArtifactIo {
        path: path.to_owned(),
        source,
    })
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn november6_allocation_native_write_forwards_nonempty_bytes_and_returns_errors() {
        struct Restore(Option<TestAllocationSources>);
        impl Drop for Restore {
            fn drop(&mut self) {
                TEST_ALLOCATION_SOURCES.set(self.0);
            }
        }
        let _restore = Restore(TEST_ALLOCATION_SOURCES.replace(Some(TestAllocationSources {
            environment: |_| None,
            identity: || Err(env::VarError::NotPresent),
            track: || {},
            session: || {},
            create_parent: |_| Ok(()),
            write: |path, bytes| {
                assert_eq!(path, Path::new("allocation-output"));
                assert_eq!(bytes, [3, 5, 8]);
                Err(io::Error::from_raw_os_error(5))
            },
        })));
        assert_eq!(
            allocation_write_bytes(Path::new("allocation-output"), vec![3, 5, 8])
                .unwrap_err()
                .raw_os_error(),
            Some(5)
        );
    }

    #[test]
    fn november_actual_allocation_entrypoints_preserve_initialization_order_identity_and_native_write_errors_without_io() {
        thread_local! {
            static EVENTS: std::cell::RefCell<Vec<&'static str>> = const { std::cell::RefCell::new(Vec::new()) };
        }
        struct Restore(Option<TestAllocationSources>, Vec<&'static str>);
        impl Drop for Restore {
            fn drop(&mut self) {
                TEST_ALLOCATION_SOURCES.set(self.0);
                EVENTS.set(std::mem::take(&mut self.1));
            }
        }
        let _lock = STATE_LOCK.lock().unwrap();
        let _reset = ResetAllocationState;
        ACTIVE.store(false, Ordering::Relaxed);
        let sources = TestAllocationSources {
            environment: |name| {
                assert_eq!(name, ARTIFACT_ENV);
                Some(Path::new("declared").join("allocation.json").into_os_string())
            },
            identity: || Err(env::VarError::NotPresent),
            track: || EVENTS.with_borrow_mut(|events| events.push("track")),
            session: || EVENTS.with_borrow_mut(|events| events.push("session")),
            create_parent: |path| {
                assert_eq!(path, Path::new("declared"));
                Err(io::Error::from_raw_os_error(5))
            },
            write: |_, _| panic!("parent creation failure must stop the native write"),
        };
        let _restore = Restore(TEST_ALLOCATION_SOURCES.replace(Some(sources)), EVENTS.take());
        prime();
        assert!(ACTIVE.load(Ordering::Relaxed));
        EVENTS.with_borrow(|events| assert_eq!(*events, ["track", "session"]));
        let path = Path::new("declared").join("allocation.json");
        assert!(
            matches!(write_worker_artifact(), Err(Error::ArtifactFormat { path: failed, message }) if failed == path && message == "missing allocation benchmark identity: environment variable not found")
        );
        assert!(
            matches!(write_json(&path, &17), Err(Error::ArtifactIo { path, source }) if path == Path::new("declared") && source.raw_os_error() == Some(5))
        );
        let artifact = br#"{"identity":"canonical","allocated_bytes":17,"allocation_count":9}"#;
        let result = crate::artifact::with_in_memory_artifact(artifact, || {
            read_artifact(&path, "native-id".to_owned(), "relative/allocation.json".to_owned())
        })
        .unwrap();
        assert_eq!(result.identity, "canonical");
        assert_eq!(result.native_identity, "native-id");
        assert_eq!(result.raw_artifacts, ["relative/allocation.json"]);
        assert_eq!(result.source, "alloc_tracker");
        assert_eq!(result.metrics["Allocated bytes"].value.as_f64().to_bits(), 17.0_f64.to_bits());
        assert_eq!(result.metrics["Allocations"].value.as_f64().to_bits(), 9.0_f64.to_bits());
    }

    #[test]
    fn gamma_round2_allocation_json_preserves_bytes_and_directory_and_serialization_errors() {
        struct Invalid;
        impl Serialize for Invalid {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("serialization refused"))
            }
        }
        let path = Path::new("allocations").join("case.json");
        let events = std::cell::RefCell::new(Vec::new());
        write_json_with(
            &path,
            &serde_json::json!({"count": 17}),
            |parent| {
                assert_eq!(parent, Path::new("allocations"));
                events.borrow_mut().push("mkdir");
                Ok(())
            },
            |output, bytes| {
                assert_eq!(output, path);
                assert_eq!(bytes, b"{\n  \"count\": 17\n}");
                events.borrow_mut().push("write");
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(*events.borrow(), ["mkdir", "write"]);
        assert!(
            matches!(write_json_with(&path, &17, |_| Err(io::Error::new(io::ErrorKind::PermissionDenied, "mkdir refused")),
            |_, _| panic!("directory failed")), Err(Error::ArtifactIo { path: failed, source }) if failed == Path::new("allocations") && source.to_string() == "mkdir refused")
        );
        assert!(
            matches!(write_json_with(&path, &Invalid, |_| Ok(()), |_, _| panic!("serialization failed")),
            Err(Error::ArtifactJson { path: failed, source }) if failed == path && source.to_string() == "serialization refused")
        );
        assert!(
            matches!(write_json_with(&path, &17, |_| Ok(()), |_, _| Err(io::Error::new(io::ErrorKind::BrokenPipe, "write refused"))),
            Err(Error::ArtifactIo { path: failed, source }) if failed == path && source.kind() == io::ErrorKind::BrokenPipe)
        );
    }

    #[test]
    fn gamma_round2_allocation_artifact_reader_preserves_counts_labels_and_identity() {
        let result = read_artifact_with(
            Path::new("allocation.json"),
            "native::case".into(),
            "raw/allocation.json".into(),
            |path| {
                assert_eq!(path, Path::new("allocation.json"));
                Ok(AllocationArtifact {
                    identity: "canonical/case".into(),
                    allocated_bytes: 317,
                    allocation_count: 11,
                })
            },
        )
        .unwrap();
        assert_eq!(result.identity, "canonical/case");
        assert_eq!(result.native_identity, "native::case");
        assert_eq!(result.source, "alloc_tracker");
        assert_eq!(result.raw_artifacts, ["raw/allocation.json"]);
        assert_eq!(result.metrics.len(), 2);
        assert_eq!(result.metrics["Allocated bytes"].value.as_f64().to_bits(), 317.0_f64.to_bits());
        assert_eq!(result.metrics["Allocations"].value.as_f64().to_bits(), 11.0_f64.to_bits());
        assert_eq!(result.metrics["Allocated bytes"].display_name, "Allocated bytes");
        assert_eq!(result.metrics["Allocations"].display_name, "Allocs");
        assert_eq!(result.metrics["Allocated bytes"].direction, MetricDirection::LowerIsBetter);
        assert_eq!(result.metrics["Allocations"].direction, MetricDirection::LowerIsBetter);
    }

    fn workload_report(iterations: Option<u64>) -> Report {
        let session = Session::new().no_stdout().no_file();
        if let Some(iterations) = iterations {
            let operation = session.operation("workload");
            let span = operation.measure_thread().iterations(iterations);
            drop(span);
        }
        session.to_report()
    }

    #[test]
    fn gamma_round2_allocation_worker_payload_validates_probe_and_iteration_count() {
        let report = workload_report(Some(1));
        let operation = report.operations().find(|(name, _)| *name == "workload").unwrap().1;
        let expected = (operation.total_bytes_allocated(), operation.total_allocations_count());
        let mut writes = 0;
        write_worker_artifact_with(
            || Some("allocation.json".into()),
            || Ok("canonical/case".into()),
            || report,
            |path, artifact| {
                writes += 1;
                assert_eq!(path, Path::new("allocation.json"));
                assert_eq!(artifact.identity, "canonical/case");
                assert_eq!((artifact.allocated_bytes, artifact.allocation_count), expected);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(writes, 1);
        for (report, expected) in [
            (
                workload_report(None),
                "benchmark 'case' did not execute its generated allocation probe",
            ),
            (
                workload_report(Some(2)),
                "benchmark 'case' recorded 2 allocation iterations instead of one",
            ),
        ] {
            assert!(matches!(write_worker_artifact_with(
                || Some("allocation.json".into()), || Ok("case".into()), || report,
                |_, _| panic!("invalid reports must not be published"),
            ), Err(Error::ArtifactFormat { path, message }) if path == Path::new("allocation.json") && message == expected));
        }
        assert!(matches!(write_worker_artifact_with(
            || Some("allocation.json".into()), || Err(env::VarError::NotPresent),
            || panic!("identity errors precede report collection"), |_, _| panic!("missing identity"),
        ), Err(Error::ArtifactFormat { path, message }) if path == Path::new("allocation.json")
            && message == format!("missing allocation benchmark identity: {}", env::VarError::NotPresent)));
        write_worker_artifact_with(
            || None,
            || panic!("disabled worker"),
            || panic!("disabled worker"),
            |_, _| panic!("disabled worker"),
        )
        .unwrap();
    }

    #[test]
    fn gamma_round2_allocation_prime_enables_only_authorized_worker_state() {
        let _lock = STATE_LOCK.lock().unwrap();
        let _reset = ResetAllocationState;
        for present in [false, true] {
            ACTIVE.store(false, Ordering::Relaxed);
            let initialized = std::cell::Cell::new(false);
            prime_with(
                |name| {
                    assert_eq!(name, ARTIFACT_ENV);
                    present.then(|| "allocation.json".into())
                },
                || {
                    assert!(!ACTIVE.load(Ordering::Relaxed));
                    initialized.set(true);
                },
            );
            assert_eq!(initialized.get(), present);
            assert_eq!(ACTIVE.load(Ordering::Relaxed), present);
        }
    }

    static STATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct ResetAllocationState;

    impl Drop for ResetAllocationState {
        fn drop(&mut self) {
            ACTIVE.store(false, Ordering::Relaxed);
            MEASURED.store(false, Ordering::Relaxed);
        }
    }

    #[test]
    fn gamma_survivor_single_allocation_boundary() {
        let _lock = STATE_LOCK.lock().unwrap();
        let _reset = ResetAllocationState;
        MEASURED.store(false, Ordering::Relaxed);
        ACTIVE.store(true, Ordering::Relaxed);
        let span = begin().unwrap();
        assert!(begin().is_none());
        drop(span);
        let report = SESSION.to_report();
        let operations = report.operations().collect::<Vec<_>>();
        assert_eq!(operations.len(), 1);
        assert_eq!(operations[0].0, "workload");
        assert_eq!(operations[0].1.total_iterations(), 1);
    }

    #[test]
    fn inactive_collection_does_not_consume_the_first_workload() {
        let _lock = STATE_LOCK.lock().unwrap();
        assert!(begin().is_none());
        assert!(begin().is_none());
        assert!(!MEASURED.load(Ordering::Relaxed));
    }
}
