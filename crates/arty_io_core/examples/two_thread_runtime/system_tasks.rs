// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::thread;

use arty_io_core::SystemTasks;

pub(super) fn runtime_system_tasks() -> SystemTasks {
    SystemTasks::new(|task| {
        drop(thread::spawn(task));
    })
}
