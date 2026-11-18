// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Supplies oversized header lengths without allocating oversized header values.

use std::cell::Cell;

thread_local! {
    static EXTRA_LENGTH: Cell<Option<(usize, usize)>> = const { Cell::new(None) };
}

pub(super) fn extra_length() -> Option<(usize, usize)> {
    EXTRA_LENGTH.take()
}

pub(crate) fn with_extra_length<T>(length: (usize, usize), action: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            EXTRA_LENGTH.set(None);
        }
    }

    assert_eq!(EXTRA_LENGTH.replace(Some(length)), None);
    let _reset = Reset;
    let result = action();
    assert_eq!(EXTRA_LENGTH.get(), None, "the request must reach header capacity validation");
    result
}
