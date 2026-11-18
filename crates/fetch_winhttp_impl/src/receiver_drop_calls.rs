// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Records destruction attempts while keeping repeated attempts away from dead storage.

use std::cell::Cell;

thread_local! {
    static ATTEMPTS: Cell<Option<usize>> = const { Cell::new(None) };
}

pub(super) fn suppress_repeated_drop() -> bool {
    ATTEMPTS.get().is_some_and(|attempts| {
        ATTEMPTS.set(Some(attempts + 1));
        attempts != 0
    })
}

pub(super) fn observe(action: impl FnOnce()) -> usize {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            ATTEMPTS.set(None);
        }
    }

    assert_eq!(ATTEMPTS.replace(Some(0)), None);
    let _reset = Reset;
    action();
    ATTEMPTS.take().unwrap()
}
