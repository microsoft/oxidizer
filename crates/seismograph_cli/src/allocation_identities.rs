// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::HashMap;

use seismograph_rallocator::callers::{Callers, EventKind};

pub(crate) type Identities = HashMap<(u64, u64), u64>;

/// Reuse the full capture's identities even when a filter hides storage changes.
pub(crate) fn from_callers(callers: &Callers) -> Option<Identities> {
    if !callers.events.iter().any(|event| event.kind == EventKind::Reallocated) {
        return None;
    }
    Some(
        callers
            .events
            .iter()
            .map(|event| ((event.recorder_thread_id, event.sequence), event.allocation_id))
            .collect(),
    )
}

#[cfg(test)]
pub(crate) fn for_capture(events: &seismograph::recorder::event::Events) -> Option<Identities> {
    if !events
        .events
        .iter()
        .any(|event| event.kind == seismograph::recorder::event::EventKind::Reallocation)
    {
        return None;
    }
    from_callers(&seismograph_rallocator::events::callers(events))
}
