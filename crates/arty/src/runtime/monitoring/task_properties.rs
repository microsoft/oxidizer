// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use seismograph_runtime::task::TaskProperties;

pub(crate) fn capture_task_properties<T: 'static>(body_size: usize) -> TaskProperties {
    let mut properties = TaskProperties::new(seismograph_runtime::task::type_descriptor::<T>());
    properties.parent = arty_executor::current_task();
    properties.body_size = body_size;
    properties
}
