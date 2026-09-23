// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::thread;

use arty_io_core::SystemTaskSpawner;

pub(super) fn runtime_spawner() -> SystemTaskSpawner {
    SystemTaskSpawner::from_fn(|task| {
        drop(thread::spawn(task));
    })
}
