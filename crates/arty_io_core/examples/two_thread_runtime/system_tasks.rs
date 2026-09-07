// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::thread;

use arty_io_core::{SystemTask, SystemTaskSpawner};

pub(super) struct RuntimeSystemTasks;

impl SystemTaskSpawner for RuntimeSystemTasks {
    fn spawn(&self, task: SystemTask) {
        drop(thread::spawn(task));
    }
}
