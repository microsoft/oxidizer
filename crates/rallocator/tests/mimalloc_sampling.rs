// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The bridge obeys the recorder's object-selection policy for both lifecycle events.

use std::alloc::{GlobalAlloc, Layout};

use seismograph::recorder::event::{EventClass, EventKind, ObjectId, Record};
use seismograph::recorder::{Configuration, EventSampling, Policy, without_recording};

rallocator::rallocator!(backend = mimalloc);

#[test]
fn sampled_lifetimes_match_the_recorders_public_selection_decision() {
    let policy = Policy {
        event_sampling: EventSampling::one_in(2).unwrap(),
        ..Policy::all(false)
    };
    seismograph::recorder(Configuration {
        allocations: policy,
        general_events: policy,
        ..Default::default()
    });
    let layout = Layout::from_size_align(257, 16).unwrap();
    let mut allocations = [std::ptr::null_mut::<u8>(); 32];
    for pointer in &mut allocations {
        // SAFETY: The layout requests an independent valid nonzero allocation.
        *pointer = unsafe { GLOBAL.alloc(layout) };
        assert!(!pointer.is_null());
        seismograph::record(EventClass::General, || {
            Some(Record::object(EventKind::ArcClone, ObjectId::from_ptr(*pointer)))
        });
    }
    for pointer in allocations {
        // SAFETY: Each block returns once with its exact original layout.
        unsafe { GLOBAL.dealloc(pointer, layout) };
    }
    without_recording(|| {
        let capture = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
        let decoded = seismograph::snapshot::decode(capture.as_bytes()).unwrap();
        for pointer in allocations {
            let selected = decoded
                .events
                .events
                .iter()
                .any(|event| event.kind == EventKind::ArcClone && event.object_id() == Some(ObjectId::from_ptr(pointer)));
            let actual = decoded
                .events
                .events
                .iter()
                .filter_map(|event| {
                    event
                        .allocation()
                        .filter(|allocation| allocation.address.get() == pointer.addr() as u64)
                        .map(|_| event.kind)
                })
                .collect::<Vec<_>>();
            let expected = if selected {
                vec![EventKind::Allocation, EventKind::Deallocation]
            } else {
                Vec::new()
            };
            assert_eq!(actual, expected, "lifecycle at {pointer:p}");
        }
    });
}
