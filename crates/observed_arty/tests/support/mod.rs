// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::mpsc;

use arty::runtime::Runtime;
use testing_aids::{TEST_TIMEOUT, is_mutation_testing};

pub(crate) fn stop_bounded(runtime: Runtime) {
    let (finished, completion) = mpsc::sync_channel(1);
    let shutdown = std::thread::spawn(move || {
        let result = runtime.stop();
        let _send_result = finished.send(result);
    });
    if is_mutation_testing() {
        completion.recv().unwrap().unwrap();
    } else {
        completion.recv_timeout(TEST_TIMEOUT).unwrap().unwrap();
    }
    shutdown.join().unwrap();
}
