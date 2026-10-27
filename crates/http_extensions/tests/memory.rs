// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public body-building behavior with caller-owned memory providers.
//!
//! Explicit reservations use different ordinary sizes to distinguish access paths, not pool boundaries.

use std::sync::{Arc, Mutex};

use bytesbuf::mem::testing::TransparentMemory;
use bytesbuf::mem::{HasMemory, Memory, OpaqueMemory};
use bytesbuf::{BytesBuf, BytesView};
use http_extensions::HttpBodyBuilder;
use thread_aware::{Relocator, Thread, ThreadAware};
use tick::Clock;

/// Records which provider instance serves each body allocation.
#[derive(Debug)]
struct ObservedMemory {
    reservations: Arc<Mutex<Vec<Reservation>>>,
    generation: usize,
    relocated: bool,
    extra_capacity: usize,
}

impl Clone for ObservedMemory {
    fn clone(&self) -> Self {
        Self {
            reservations: Arc::clone(&self.reservations),
            generation: self.generation + 1,
            relocated: self.relocated,
            extra_capacity: self.extra_capacity,
        }
    }
}

impl ThreadAware for ObservedMemory {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {
        self.relocated = true;
    }
}

impl Memory for ObservedMemory {
    #[expect(clippy::unwrap_used, reason = "test provider must fail if its observation lock is poisoned")]
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        self.reservations.lock().unwrap().push(Reservation {
            bytes: min_bytes,
            generation: self.generation,
            relocated: self.relocated,
        });
        TransparentMemory::new().reserve(min_bytes + self.extra_capacity)
    }
}

/// Distinguishes allocations made by the original, cloned, and relocated providers.
#[derive(Debug, PartialEq, Eq)]
struct Reservation {
    bytes: usize,
    generation: usize,
    relocated: bool,
}

/// Separates provider cloning and relocation from allocations through the original builder.
#[test]
fn bodies_keep_the_provider_clone_and_relocation_behavior() {
    let reservations = Arc::new(Mutex::new(Vec::new()));
    let builder = HttpBodyBuilder::new(
        OpaqueMemory::new(ObservedMemory {
            reservations: Arc::clone(&reservations),
            generation: 0,
            relocated: false,
            extra_capacity: 0,
        }),
        &Clock::new_frozen(),
    );
    assert_eq!(BytesView::try_from(builder.text("original")).unwrap(), b"original");

    let mut cloned = builder.clone();
    assert_eq!(BytesView::try_from(cloned.slice(b"clone")).unwrap(), b"clone");
    _ = Relocator::between_threads().relocate(&mut cloned);
    assert_eq!(BytesView::try_from(cloned.text("moved")).unwrap(), b"moved");

    // Relocating the clone must not replace or relocate the original provider.
    assert_eq!(builder.reserve(3).capacity(), 3);
    assert_eq!(builder.memory().reserve(2).capacity(), 2);
    assert_eq!(
        *reservations.lock().unwrap(),
        [
            Reservation {
                bytes: 8,
                generation: 0,
                relocated: false
            },
            Reservation {
                bytes: 5,
                generation: 1,
                relocated: false
            },
            Reservation {
                bytes: 5,
                generation: 1,
                relocated: true
            },
            Reservation {
                bytes: 3,
                generation: 0,
                relocated: false
            },
            Reservation {
                bytes: 2,
                generation: 1,
                relocated: false
            },
        ]
    );
}

/// Makes sure that provider capacity does not become body data.
#[test]
fn body_lengths_do_not_include_extra_provider_capacity() {
    // Exact reservations and a deliberately oversized block exercise both provider choices.
    for extra_capacity in [0, 4096] {
        let reservations = Arc::new(Mutex::new(Vec::new()));
        let builder = HttpBodyBuilder::with_custom_memory(
            ObservedMemory {
                reservations: Arc::clone(&reservations),
                generation: 0,
                relocated: false,
                extra_capacity,
            },
            &Clock::new_frozen(),
        );
        let body = builder.slice(b"payload");

        assert_eq!(body.content_length(), Some(7));
        assert_eq!(BytesView::try_from(body).unwrap(), b"payload");
        assert_eq!(builder.reserve(7).capacity(), 7 + extra_capacity);
        assert_eq!(
            reservations.lock().unwrap().iter().map(|entry| entry.bytes).collect::<Vec<_>>(),
            [7, 7]
        );
    }
}
