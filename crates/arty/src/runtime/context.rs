// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::rc::Rc;

use observed::Sink;
use thread_aware::Thread;
use tick::Clock;

use crate::io::Io;
use crate::runtime::dispatch::DispatcherClient;
use crate::task::scheduler::Scheduler;

/// Capabilities assembled during worker initialization.
#[derive(Debug)]
#[non_exhaustive]
pub(crate) struct RuntimeBuiltins {
    pub(crate) task_scheduler: Scheduler,
    pub(crate) clock: Clock,
    pub(crate) io: Io,
    pub(crate) thread: Thread,
    pub(crate) sink: Sink,
}

impl RuntimeBuiltins {
    pub(in crate::runtime) fn new(dispatcher: &Rc<DispatcherClient>, clock: Clock, io: Io, thread: Thread, sink: Sink) -> Self {
        Self {
            task_scheduler: Scheduler::new(dispatcher.as_ref().clone(), thread.clone()),
            clock,
            io,
            thread,
            sink,
        }
    }
}
