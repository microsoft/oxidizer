// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use seismograph_runtime::task::TaskProperties;

pub(crate) fn capture_task_properties(properties: TaskProperties) -> TaskProperties {
    let properties = properties.parent(arty_executor::current_task());
    match arty_executor::current_origin() {
        Some((runtime, worker)) => properties.origin(runtime, worker),
        None => properties,
    }
}
