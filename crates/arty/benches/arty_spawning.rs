// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Whole-process spawning measurements, including runtime workers and live monitor delivery.
//!
//! Run natively for wall time and process CPU time. For worker-inclusive instruction
//! counts run the same executable under Callgrind with `--instr-atstart=no`,
//! `--collect-atstart=yes`, `--separate-threads=yes`, and pass `--callgrind` here.
//! The process-wide instrumentation window excludes runtime setup, warm-up, and teardown.
//! Paired with the Criterion spawn/yield scenarios in `arty_scheduling.rs`.
//! Client requests require Valgrind headers and libclang at build time. For
//! nonstandard installations set `VALGRIND_REQUESTS_VALGRIND_INCLUDE` and
//! `LIBCLANG_PATH`. Native timings never come from execution under Valgrind.

#[cfg(target_os = "linux")]
#[path = "support/spawning.rs"]
mod spawning;

#[cfg(all(target_os = "linux", feature = "seismograph"))]
fn main() -> spawning::Result<()> {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    use seismograph::recorder::runtime::RuntimeId;
    use seismograph_runtime::live::{Measurements, SubscriptionOptions, subscribe};

    spawning::run(|runtime, mode, batches: Arc<AtomicU64>| {
        if mode == spawning::Monitoring::Off {
            return Ok(None);
        }
        let (ready, started) = mpsc::sync_channel(1);
        let first = AtomicBool::new(true);
        let subscription = subscribe(
            RuntimeId::from_raw(runtime.id().get()).expect("Arty exposes a registered nonzero runtime identity"),
            SubscriptionOptions {
                measurements: match mode {
                    spawning::Monitoring::State => Measurements::State,
                    spawning::Monitoring::Timing => Measurements::PollTiming,
                    spawning::Monitoring::Off => unreachable!("the disabled case returned above"),
                },
                refresh_interval: Duration::from_millis(50),
                ..SubscriptionOptions::default()
            },
            move |batch| {
                std::hint::black_box(&batch);
                batches.fetch_add(1, Ordering::Relaxed);
                if first.swap(false, Ordering::Relaxed) {
                    let _disconnected = ready.send(());
                }
            },
        )?;
        started.recv_timeout(Duration::from_secs(5))?;
        Ok(Some(subscription))
    })
}

#[cfg(all(target_os = "linux", not(feature = "seismograph")))]
fn main() -> spawning::Result<()> {
    spawning::run(|_, mode, _| {
        if mode != spawning::Monitoring::Off {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "live monitor modes require Arty's seismograph Cargo feature",
            )
            .into());
        }
        Ok(())
    })
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "whole-process spawning measurements require the Linux process CPU clock",
    ))
}
