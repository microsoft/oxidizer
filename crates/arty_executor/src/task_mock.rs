// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::task;

use mockall::mock;

use crate::{TaskCreatedReporter, TaskLinks, TaskOptions, TypeErasedTask, WakeSignal};

// Mockall is not able to express all methods on the trait (due to generics deficiency), so we mock
// similar-enough methods that it does know how to mock and simply call these from a manual
// implementation of the trait that translates between the two forms.
mock! {
    #[derive(Debug)]
    pub TypeErasedTask {
        fn poll(self: Pin<&Self>) -> task::Poll<()>;
        fn abort(self: Pin<&Self>);
        fn initialize(self: Pin<&Self>, wake_signal: WakeSignal);
        // Intentionally missing: inspect_waker_backtraces
    }
}

impl TypeErasedTask for MockTypeErasedTask {
    fn poll(self: Pin<&Self>) -> task::Poll<()> {
        self.poll()
    }

    fn created_reporter(&self) -> Option<TaskCreatedReporter> {
        TaskOptions::default().created_reporter(observed::Sink::noop())
    }

    fn links(&self) -> &TaskLinks {
        panic!("mock task links are not used")
    }

    fn clear_queued_notification(&self) {}

    fn abort(self: Pin<&Self>) {
        self.abort();
    }

    fn initialize(self: Pin<&Self>, wake_signal: WakeSignal) {
        self.initialize(wake_signal);
    }

    #[cfg(all(debug_assertions, test))]
    fn inspect_waker_backtraces(&self, _f: &mut dyn FnMut(&std::backtrace::Backtrace)) {}
}
