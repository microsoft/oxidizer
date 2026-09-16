// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Snapshot allocations retain ordinary ownership lifetimes without entering telemetry.

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use seismograph::snapshot::{SnapshotContext, SnapshotOptions, Source, SourceData, SourceId};

    rallocator::rallocator!();

    static MODE: AtomicUsize = AtomicUsize::new(0);
    static ESCAPED: Mutex<Option<Vec<u8>>> = Mutex::new(None);
    static SOURCE: Source = Source::new(SourceId::new(u64::MAX), "soundness-test", 1, capture);

    fn capture(_context: SnapshotContext<'_>) -> Result<SourceData, seismograph::Error> {
        match MODE.load(Ordering::Relaxed) {
            0 => Err(seismograph::Error::new("injected source failure")),
            1 => panic!("{}", String::from("injected source panic")),
            _ => {
                *ESCAPED.lock().unwrap() = Some(vec![0x5a; 3 * 1024 * 1024]);
                SourceData::copy_from(&[1, 2, 3])
            }
        }
    }

    #[test]
    fn source_errors_panics_and_retained_data_outlive_capture() {
        // Keep source registration in this test, after the successful capture.
        let snapshot = seismograph::snapshot(SnapshotOptions::default()).unwrap();
        assert!(!seismograph::snapshot::snapshot_arena_active());
        std::thread::spawn(move || {
            assert!(!seismograph::snapshot::snapshot_arena_active());
            let decoded = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap();
            assert!(decoded.sources.iter().any(|source| source.id == seismograph_rallocator::source::ID));
            drop(snapshot);
        })
        .join()
        .unwrap();

        seismograph::snapshot::register_source(&SOURCE);

        let error = seismograph::snapshot(SnapshotOptions::default()).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("seismograph source {} failed: injected source failure", u64::MAX)
        );
        drop(error);

        MODE.store(1, Ordering::Relaxed);
        let payload = std::panic::catch_unwind(|| seismograph::snapshot(SnapshotOptions::default())).unwrap_err();
        assert_eq!(payload.downcast_ref::<String>().map(String::as_str), Some("injected source panic"));
        drop(payload);

        MODE.store(2, Ordering::Relaxed);
        let snapshot = seismograph::snapshot(SnapshotOptions::default()).unwrap();
        drop(snapshot);
        let retained = ESCAPED.lock().unwrap().take().unwrap();
        std::thread::spawn(move || assert!(retained.iter().all(|byte| *byte == 0x5a)))
            .join()
            .unwrap();
    }
}
