// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::mpsc;
use std::time::Duration;

use arty::runtime::{Runtime, WorkersPolicy};

pub(crate) const TEST_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) fn runtime(workers: usize) -> Runtime {
    Runtime::builder().workers(WorkersPolicy::exactly(workers)).build().unwrap()
}

pub(crate) fn stop_bounded(runtime: Runtime) {
    let (finished, completion) = mpsc::sync_channel(1);
    let shutdown = std::thread::spawn(move || {
        let result = runtime.stop();
        let _send_result = finished.send(result);
    });
    completion.recv_timeout(TEST_TIMEOUT).unwrap().unwrap();
    shutdown.join().unwrap();
}
