// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Worker routing, submission capabilities, and stop coordination.

mod dispatcher_client;
mod dispatcher_core;
pub(crate) use dispatcher_client::DispatcherClient;
pub(crate) use dispatcher_core::WorkerIndex;
pub(super) use dispatcher_core::{DispatcherCore, WorkerEndpoint};

#[cfg(test)]
fn test_threads(count: usize) -> Vec<thread_aware::Thread> {
    let builder = thread_aware::ThreadBuilder::default();
    (0..count)
        .map(|_| {
            let builder = builder.clone();
            std::thread::spawn(move || builder.build(std::thread::current().id()))
                .join()
                .unwrap()
        })
        .collect()
}
