// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public task panic-payload disposal budget contracts.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use arty_executor::discard_task_panic_payload;

#[test]
fn gamma_survivor_panic_payload_chain_finishes_on_last_allowed_disposal() {
    struct Payload {
        remaining_panics: usize,
        drops: Arc<AtomicUsize>,
    }

    impl Drop for Payload {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::Relaxed);
            if self.remaining_panics != 0 {
                std::panic::panic_any(Self {
                    remaining_panics: self.remaining_panics - 1,
                    drops: Arc::clone(&self.drops),
                });
            }
        }
    }

    let drops = Arc::new(AtomicUsize::new(0));
    discard_task_panic_payload(Box::new(Payload {
        remaining_panics: 31,
        drops: Arc::clone(&drops),
    }));
    assert_eq!(drops.load(Ordering::Relaxed), 32);
}
