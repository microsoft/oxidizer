// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::Cell;

use seismograph::recorder::runtime::TaskId;
use seismograph_runtime::task::EventContext;

thread_local! {
    static EVENTS: Cell<Option<EventContext>> = const { Cell::new(None) };
}

pub(crate) fn current_events() -> Option<EventContext> {
    EVENTS.with(Cell::get)
}

pub(crate) fn set_events(events: Option<EventContext>) {
    EVENTS.with(|slot| slot.set(events));
}

pub(crate) fn current_task() -> Option<TaskId> {
    arty_executor::current_task()
}
