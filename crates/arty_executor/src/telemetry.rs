// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use data_privacy::classified;
use observed::{Sink, emit, event};

use crate::TaskPlacement;

static SYSTEM_METADATA: data_privacy::DataClass = data_privacy::DataClass::new("arty", "SystemMetadata");

#[classified(SYSTEM_METADATA)]
#[derive(Clone, Copy)]
struct PlacementLabel(&'static str);

pub(crate) fn report_created(sink: &Sink, placement: TaskPlacement) {
    let placement = match placement {
        TaskPlacement::Any => "any",
        TaskPlacement::SameThread => "same_thread",
        TaskPlacement::SpecificThread => "specific_thread",
    };
    emit!(
        sink,
        TaskCreated {
            placement: PlacementLabel(placement)
        }
    );
}

pub(crate) fn report_succeeded(sink: &Sink, polls: u64) {
    emit!(sink, TaskSucceeded { polls });
}

pub(crate) fn report_panicked(sink: &Sink, polls: u64) {
    emit!(sink, TaskPanicked { polls });
}

#[event("arty.rt.task.spawned")]
#[counter(name = "arty.rt.task.spawned")]
struct TaskCreated {
    #[dimension(metric = "placement")]
    placement: PlacementLabel,
}

#[event("arty.rt.task.succeeded")]
#[counter(name = "arty.rt.task.succeeded")]
#[histogram(polls, name = "arty.rt.task.succeeded.polls", unit = "{poll}")]
struct TaskSucceeded {
    #[unredacted]
    polls: u64,
}

#[event("arty.rt.task.panicked")]
#[error("task panicked")]
#[counter(name = "arty.rt.task.panicked")]
#[histogram(polls, name = "arty.rt.task.panicked.polls", unit = "{poll}")]
struct TaskPanicked {
    #[unredacted]
    polls: u64,
}
