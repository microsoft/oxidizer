// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Equivalence contracts for the concurrent CPU-bound throughput fixture.
//!
//! These assert what the throughput numbers depend on: that every target runs
//! the identical deterministic CPU work, returns the identical responses for
//! the identical seeds, and completes every request of a concurrent batch.

#![cfg(not(miri))]
#![allow(dead_code, reason = "the shared fixture supports the harness and regular tests")]
#![allow(missing_docs, reason = "fixture tests need no API documentation")]
include!("../benches/common/throughput_scenarios.rs");

#[test]
fn the_cpu_work_is_deterministic_seed_dependent_and_scaled() {
    assert_cpu_work_is_deterministic_and_scaled();
}

#[test]
fn every_target_computes_the_identical_cpu_result() {
    assert_equivalent();
}

/// The handler-only control must produce exactly what the frameworks produce,
/// so the application floor it reports is the same work the frameworks run.
#[test]
fn the_control_row_runs_the_same_work_as_the_frameworks() {
    for workload in Workload::ALL {
        let control = run_single_worker_batch(Target::HandlerOnly, workload, 16);
        for framework in Target::FRAMEWORKS {
            let measured = run_single_worker_batch(framework, workload, 16);
            assert_eq!(
                measured.checksum,
                control.checksum,
                "{} disagreed with the handler-only control on the {} workload",
                framework.name(),
                workload.name()
            );
            assert_eq!(measured.requests, control.requests);
        }
    }
}

/// A concurrent batch must complete every request it was given, whichever
/// order the runtime interleaved its slots in.
#[test]
fn a_concurrent_batch_completes_every_request_in_any_interleaving() {
    const SLOTS: usize = 4;
    const REQUESTS_PER_SLOT: usize = 8;

    for target in Target::ALL {
        let sequential = run_single_worker_batch(target, Workload::Light, SLOTS * REQUESTS_PER_SLOT);
        let expected: u64 = (0..SLOTS * REQUESTS_PER_SLOT)
            .map(|index| {
                let seed = seed_for(0, index, SLOTS * REQUESTS_PER_SLOT);
                digest(200, work_body(Workload::Light, seed).as_bytes())
            })
            .fold(0_u64, u64::wrapping_add);
        assert_eq!(sequential.checksum, expected, "{} lost or repeated a request", target.name());
        assert!(
            sequential.elapsed > Duration::ZERO,
            "{} reported a zero-length batch",
            target.name()
        );
    }
}
