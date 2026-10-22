// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Allocator-backed capture cleanup through source failure and unwinding.
#![cfg(not(miri))]

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use seismograph::snapshot::{SnapshotContext, SnapshotOptions, Source, SourceData, SourceId};

rallocator::rallocator!();

static MODE: AtomicUsize = AtomicUsize::new(0);
static CACHE: Mutex<Option<Vec<u8>>> = Mutex::new(None);
static SOURCE: Source = Source::new(SourceId::new(0x534f_554e_444e_4553), "failure-recovery", 1, capture);

#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "the test source deliberately injects failure and an owned panic"
)]
fn capture(_context: SnapshotContext<'_>) -> Result<SourceData, seismograph::Error> {
    *CACHE.lock().unwrap() = Some(vec![0x5a; 16_384]);
    match MODE.load(Ordering::Relaxed) {
        0 => Err(seismograph::Error::new("injected source failure")),
        1 => std::panic::panic_any(String::from("injected owned source panic")),
        _ => {
            let mut data = SourceData::zeroed(4)?;
            data.as_mut_bytes().copy_from_slice(b"pass");
            Ok(data)
        }
    }
}

#[test]
fn allocator_backed_capture_recovers_after_source_error_and_panic() {
    let retained = vec![0xa5; 32_768];
    seismograph::snapshot::register_source(&SOURCE);
    seismograph::recorder(seismograph::recorder::Configuration {
        allocations: seismograph::recorder::RecordingPolicy::all(false),
        ..Default::default()
    });
    for (mode, message) in [(0, "injected source failure"), (1, "source panicked")] {
        MODE.store(mode, Ordering::Relaxed);
        let error = seismograph::snapshot(SnapshotOptions::default()).unwrap_err();
        assert!(error.to_string().contains(message), "{error}");
        let cache = CACHE.lock().unwrap().take().unwrap();
        assert_eq!(cache, vec![0x5a; 16_384]);
        drop(cache);
        assert_eq!(retained, vec![0xa5; 32_768]);
    }
    MODE.store(2, Ordering::Relaxed);
    let recording = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let decoded = seismograph::snapshot::decode(recording.as_bytes()).unwrap();
    assert_eq!(
        decoded
            .sources
            .iter()
            .find(|source| source.id == SourceId::new(0x534f_554e_444e_4553))
            .unwrap()
            .data,
        b"pass"
    );
    let native = decoded
        .sources
        .iter()
        .find(|source| source.id == seismograph_rallocator::source::ID)
        .unwrap();
    assert_eq!(native.schema_version, 3);
    assert!(seismograph_rallocator::decode(&native.data).unwrap().global.local_limit_bytes > 0);
    assert_eq!(CACHE.lock().unwrap().take().unwrap(), vec![0x5a; 16_384]);
    drop(retained);
    seismograph::recorder(seismograph::recorder::Configuration::default());
    seismograph::snapshot(SnapshotOptions::default()).unwrap();
    CACHE.lock().unwrap().take();
}
