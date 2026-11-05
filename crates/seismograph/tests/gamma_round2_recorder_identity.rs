// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Owns its process-wide recorder identity sequence; no recording is enabled.

use seismograph::recorder::thread_id;

#[test]
fn gamma_round2_fresh_thread_recorders_receive_consecutive_stable_ids() {
    let first = std::thread::spawn(|| {
        let id = thread_id();
        assert_eq!(thread_id(), id);
        id
    })
    .join()
    .unwrap();
    let second = std::thread::spawn(thread_id).join().unwrap();
    assert_eq!(second.get(), first.get() + 1);
}
