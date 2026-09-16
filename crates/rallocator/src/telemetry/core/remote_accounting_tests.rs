// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::{RemoteDrainClaim, with_telemetry_suppressed};

#[test]
fn claim_updates_pending_before_processing_and_preserves_modulo() {
    let available = AtomicBool::new(true);
    let pending = AtomicUsize::new(2);
    RemoteDrainClaim::if_available(&available, &pending).unwrap().record(2);
    assert_eq!(pending.load(Ordering::Relaxed), 0);
    RemoteDrainClaim::if_available(&available, &pending).unwrap().record(1);
    assert_eq!(pending.load(Ordering::Relaxed), usize::MAX);
}

#[test]
fn unavailable_claim_does_not_become_a_late_claim() {
    let available = AtomicBool::new(false);
    let pending = AtomicUsize::new(2);
    let first = RemoteDrainClaim::if_available(&available, &pending);
    available.store(true, Ordering::Release);
    assert!(first.is_none());
    RemoteDrainClaim::if_available(&available, &pending).unwrap().record(1);
    assert_eq!(pending.load(Ordering::Relaxed), 1);
}

#[test]
fn normal_claim_ignores_recording_suppression() {
    let available = AtomicBool::new(true);
    let pending = AtomicUsize::new(7);
    with_telemetry_suppressed(|| {
        RemoteDrainClaim::if_available(&available, &pending).unwrap().record(7);
    });
    assert_eq!(pending.load(Ordering::Relaxed), 0);
}

// Exact global deltas need a separate process, not a mutex that other allocator
// tests do not take. Miri covers the private-cell and real slab fixtures instead.
#[cfg(all(not(miri), feature = "tuning-telemetry"))]
#[test]
fn isolated_lifetime_accounting_survives_recording_transitions() {
    use super::{
        AGGREGATES_AVAILABLE, PENDING_REMOTE_BLOCKS, begin_remote_free, finish_remote_free, record_allocation, record_remote_drain,
        record_remote_retired_free, remote_drain_claim, stats,
    };
    use crate::tuning_telemetry::TuningTelemetry;

    const CHILD: &str = "RALLOCATOR_TEST_REMOTE_ACCOUNTING_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "telemetry::core::remote_accounting_tests::isolated_lifetime_accounting_survives_recording_transitions",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        return;
    }

    let values = || {
        [
            super::super::remote_counts::frees(),
            PENDING_REMOTE_BLOCKS.load(Ordering::Relaxed),
            super::super::remote_counts::pushes_in_progress(),
            super::super::remote_counts::drained(),
        ]
    };
    // Libtest uses System here. Before registering an aggregate shard, neither
    // lifetime reporting nor opt-in recording has been activated by this test.
    assert!(!AGGREGATES_AVAILABLE.load(Ordering::Relaxed));
    let availability = begin_remote_free();
    finish_remote_free(availability);
    record_remote_drain();
    record_remote_retired_free();
    assert_eq!(values(), [0; 4]);

    record_allocation(1);
    let availability = begin_remote_free();
    assert_eq!(values(), [1, 1, 1, 0]);
    seismograph::recorder(seismograph::recorder::Configuration::default());
    TuningTelemetry::enable();
    TuningTelemetry::disable();
    finish_remote_free(availability);
    assert_eq!(values(), [1, 1, 0, 0]);
    record_remote_drain();
    assert_eq!(values(), [1, 0, 0, 1]);
    with_telemetry_suppressed(record_remote_retired_free);
    assert_eq!(values(), [1, 0, 0, 1]);
    record_remote_retired_free();
    assert_eq!(values(), [2, 0, 0, 1]);

    for _ in 0..7 {
        let availability = begin_remote_free();
        finish_remote_free(availability);
    }
    with_telemetry_suppressed(|| remote_drain_claim().unwrap().record(7));
    assert_eq!(values(), [9, 0, 0, 8]);

    let before = values();
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                for _ in 0..32 {
                    let availability = begin_remote_free();
                    // Exercise accounting only; actual publication and detached
                    // ownership are covered by the slab and concurrent-list tests.
                    finish_remote_free(availability);
                    remote_drain_claim().unwrap().record(1);
                }
            });
        }
        scope.spawn(|| {
            for _ in 0..16 {
                TuningTelemetry::enable();
                assert!(TuningTelemetry::snapshot_if_active().is_some());
                TuningTelemetry::disable();
                assert!(TuningTelemetry::snapshot_if_active().is_none());
                let stats = stats().unwrap();
                assert!(stats.remote_frees >= before[0]);
                assert!(stats.drained_remote_blocks >= before[3]);
            }
        });
    });
    assert_eq!(values(), [before[0] + 128, 0, 0, before[3] + 128]);
    assert!(AGGREGATES_AVAILABLE.load(Ordering::Relaxed));
}
