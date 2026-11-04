// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicUsize, Ordering};

use thread_aware::{Relocator, ThreadAware};

use crate::arc::{Arc, PerThread};

#[test]
fn warm_relocation_drops_unregistered_value_outside_storage_lock() {
    // The second relocation destroys the initial value synchronously on this test thread.
    // The counter records only the lock observation and publishes no other memory,
    // so both atomic operations use relaxed ordering.
    #[derive(Clone)]
    struct Probe {
        selected: StdArc<std::sync::Mutex<Option<Arc<Self, PerThread>>>>,
        unlocked_drops: StdArc<AtomicUsize>,
    }

    impl Drop for Probe {
        fn drop(&mut self) {
            if let Some(selected) = self.selected.lock().unwrap().as_ref()
                && selected.state.shared.inner.try_read().is_ok()
            {
                self.unlocked_drops.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    let selected = StdArc::new(std::sync::Mutex::new(None));
    let unlocked_drops = StdArc::new(AtomicUsize::new(0));
    let mut original = Arc::<Probe, PerThread>::from_unaware(Probe {
        selected: StdArc::clone(&selected),
        unlocked_drops: StdArc::clone(&unlocked_drops),
    });
    let mut relocated = original.clone();
    let (_, destination) = Relocator::between_threads().relocate(&mut ());
    // With no source, preparation leaves the initial allocation unregistered.
    // Only original retains it after this call.
    relocated.relocate(None, &destination);
    // Retain the destination handle so the original probe can inspect its shared lock.
    *selected.lock().unwrap() = Some(relocated);

    original.relocate(None, &destination);
    let observed = unlocked_drops.load(Ordering::Relaxed);
    // Break the ownership cycle between selected and the destination handle before an assertion can panic.
    let retained = selected.lock().unwrap().take();
    drop(retained);
    assert_eq!(observed, 1);
}
