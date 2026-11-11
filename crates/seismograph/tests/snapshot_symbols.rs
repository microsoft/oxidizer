// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Captures native symbols through the public snapshot API without a source.
#![cfg(not(miri))]

use std::collections::BTreeSet;

use seismograph::recorder::event::{EventClass, EventKind, ObjectId, Record};
use seismograph::recorder::{Configuration, Policy};
use seismograph::snapshot::{EventBufferDisposition, SnapshotOptions};

// Keep a known application frame on the stack for native symbol resolution.
#[inline(never)]
fn record_snapshot_symbol_marker() {
    seismograph::record(EventClass::General, || {
        Some(Record::object(EventKind::MutexAccess, ObjectId::new(17)))
    });
    std::hint::black_box(());
}

#[test]
fn snapshots_preserve_event_symbols_without_an_allocator_or_runtime_source() {
    seismograph::recorder(Configuration {
        general_events: Policy::all(true),
        ..Configuration::default()
    });
    record_snapshot_symbol_marker();
    seismograph::recorder(Configuration::default());

    for event_buffers in [EventBufferDisposition::Retain, EventBufferDisposition::Stop] {
        let encoded = seismograph::snapshot(SnapshotOptions { event_buffers }).unwrap();
        let decoded = seismograph::snapshot::decode(encoded.as_bytes()).unwrap();

        assert!(decoded.sources.is_empty());
        assert!(!decoded.events.events.is_empty());
        let frames = decoded
            .events
            .events
            .iter()
            .flat_map(|event| &event.call_stack)
            .copied()
            .filter(|address| address.get() != 0)
            .collect::<BTreeSet<_>>();
        assert!(!frames.is_empty());
        assert_eq!(
            decoded.symbols.iter().map(|symbol| symbol.address).collect::<Vec<_>>(),
            frames.into_iter().collect::<Vec<_>>()
        );
        assert!(
            decoded.symbols.iter().any(|symbol| {
                symbol
                    .name
                    .as_deref()
                    .is_some_and(|name| name.contains("record_snapshot_symbol_marker"))
                    && symbol.filename.as_deref().is_some_and(|path| path.ends_with("snapshot_symbols.rs"))
                    && symbol.line.is_some()
            }),
            "capture must store the marker's native function name and source location"
        );
    }
}
