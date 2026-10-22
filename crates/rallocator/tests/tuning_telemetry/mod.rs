// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

const RECORDING_CLOSED: usize = 1 << (usize::BITS - 1);
const RECORDER_COUNT: usize = RECORDING_CLOSED - 1;

fn admitted_count(state: usize) -> Option<usize> {
    if state & RECORDING_CLOSED != 0 || state == RECORDER_COUNT {
        None
    } else {
        Some(state + 1)
    }
}

mod models;
