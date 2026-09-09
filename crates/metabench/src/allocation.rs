// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{env, fs};

use alloc_tracker::{ProcessSpan, Session};
use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::report::{Metric, MetricDirection, NativeResult};

pub(crate) const ARTIFACT_ENV: &str = "METABENCH_INTERNAL_ALLOCATION_ARTIFACT";
pub(crate) const IDENTITY_ENV: &str = "METABENCH_INTERNAL_ALLOCATION_IDENTITY";

static ACTIVE: LazyLock<bool> = LazyLock::new(|| env::var_os(ARTIFACT_ENV).is_some());
static MEASURED: AtomicBool = AtomicBool::new(false);
static SESSION: LazyLock<Session> = LazyLock::new(|| Session::new().no_stdout().no_file());

#[derive(Debug, Deserialize, Serialize)]
struct AllocationArtifact {
    identity: String,
    allocated_bytes: u64,
    allocation_count: u64,
}

#[inline]
/// Begins measuring the first workload invocation in an allocation worker.
#[doc(hidden)]
pub fn begin() -> Option<ProcessSpan> {
    (*ACTIVE && !MEASURED.swap(true, Ordering::Relaxed)).then(|| {
        let operation = SESSION.operation("workload");
        operation.measure_process().iterations(1)
    })
}

pub(crate) fn write_worker_artifact() -> Result<(), Error> {
    let Some(path) = env::var_os(ARTIFACT_ENV).map(PathBuf::from) else {
        return Ok(());
    };
    let identity = env::var(IDENTITY_ENV).map_err(|error| Error::ArtifactFormat {
        path: path.clone(),
        message: format!("missing allocation benchmark identity: {error}"),
    })?;
    let report = SESSION.to_report();
    let operation = report
        .operations()
        .find_map(|(name, operation)| (name == "workload").then_some(operation))
        .ok_or_else(|| Error::ArtifactFormat {
            path: path.clone(),
            message: format!("benchmark '{identity}' did not execute its instrumented workload"),
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
    write_json(
        &path,
        &AllocationArtifact {
            identity,
            allocated_bytes: operation.total_bytes_allocated(),
            allocation_count: operation.total_allocations_count(),
        },
    )
}

pub(crate) fn read_artifact(path: &Path, native_identity: String, relative_path: String) -> Result<NativeResult, Error> {
    let artifact: AllocationArtifact = read_json(path)?;
    Ok(NativeResult {
        identity: artifact.identity,
        native_identity,
        source: "alloc_tracker".to_owned(),
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
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| Error::ArtifactIo {
            path: parent.to_owned(),
            source,
        })?;
    }
    let bytes = serde_json::to_vec_pretty(value).map_err(|source| Error::ArtifactJson {
        path: path.to_owned(),
        source,
    })?;
    fs::write(path, bytes).map_err(|source| Error::ArtifactIo {
        path: path.to_owned(),
        source,
    })
}

fn read_json<T>(path: &Path) -> Result<T, Error>
where
    T: for<'de> Deserialize<'de>,
{
    let file = fs::File::open(path).map_err(|source| Error::ArtifactIo {
        path: path.to_owned(),
        source,
    })?;
    serde_json::from_reader(file).map_err(|source| Error::ArtifactJson {
        path: path.to_owned(),
        source,
    })
}
