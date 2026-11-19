// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Publication control without sampling a process clock.

use seismograph_rallocator::native::{publication_enabled, set_publication_enabled};

#[test]
fn gamma_survivor_publication_toggle_applies_both_values() {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            set_publication_enabled(self.0);
        }
    }
    let _restore = Restore(publication_enabled());
    set_publication_enabled(false);
    assert!(!publication_enabled());
    set_publication_enabled(true);
    assert!(publication_enabled());
}
