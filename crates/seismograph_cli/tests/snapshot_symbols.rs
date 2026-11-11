// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Saved common symbols reach the public HTML command without a source plugin.
#![cfg(not(miri))]

use std::process::{Command, Stdio};

use seismograph::recorder::alloc::{Allocation, AllocationId, EventThreadId, HeapId, HeapKind};
use seismograph::recorder::event::{Address, EventClass, Record};
use seismograph::recorder::{Configuration, Policy};
use seismograph::snapshot::{EventBufferDisposition, SnapshotOptions};

// Keep an application frame whose symbol must survive the offline CLI process.
#[inline(never)]
fn record_allocation_symbol_marker() {
    seismograph::record(EventClass::Allocation, || {
        Some(Record::allocation(Allocation {
            allocation_id: AllocationId::new(0x1000),
            event_thread_id: EventThreadId::new(0),
            heap_id: HeapId::new(0),
            heap_kind: HeapKind::General,
            freed_after_heap_release: false,
            address: Address::new(0x1000),
            size: 64,
            alignment: 8,
        }))
    });
    std::hint::black_box(());
}

#[test]
fn html_uses_symbols_resolved_by_the_producing_process_without_a_runtime_source() {
    seismograph::recorder(Configuration {
        allocations: Policy::all(true),
        ..Configuration::default()
    });
    record_allocation_symbol_marker();
    let capture = seismograph::snapshot(SnapshotOptions {
        event_buffers: EventBufferDisposition::Stop,
    })
    .unwrap();
    assert!(seismograph::snapshot::decode(capture.as_bytes()).unwrap().sources.is_empty());
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("capture.seismograph");
    let output = directory.path().join("report.html");
    capture.write_file(&input).unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_seismograph"))
        .args(["snapshot", "html"])
        .arg(&input)
        .arg(&output)
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let html = std::fs::read_to_string(output).unwrap();
    assert!(
        html.contains("record_allocation_symbol_marker") && html.contains("snapshot_symbols.rs"),
        "the report must use function names and source locations stored by the producer"
    );
}
