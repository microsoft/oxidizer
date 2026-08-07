// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::task;

use mockall::mock;

use crate::{TypeErasedTask, WakeSignal};

// Mockall is not able to express all methods on the trait (due to generics deficiency), so we mock
// similar-enough methods that it does know how to mock and simply call these from a manual
// implementation of the trait that translates between the two forms.
mock! {
    #[derive(Debug)]
    pub TypeErasedTask {
        fn poll(self: Pin<&Self>) -> task::Poll<()>;
        fn is_inert(&self) -> bool;
        fn consume_awakened(&self) -> bool;
        fn abort(self: Pin<&Self>);
        unsafe fn initialize(self: Pin<&Self>, wake_signal: WakeSignal);
        // Intentionally missing: inspect_waker_backtraces
    }
}

impl TypeErasedTask for MockTypeErasedTask {
    fn poll(self: Pin<&Self>) -> task::Poll<()> {
        self.poll()
    }

    fn is_inert(&self) -> bool {
        self.is_inert()
    }

    fn consume_awakened(&self) -> bool {
        self.consume_awakened()
    }

    fn abort(self: Pin<&Self>) {
        self.abort();
    }

    unsafe fn initialize(self: Pin<&Self>, wake_signal: WakeSignal) {
        // SAFETY: Forwarding safety guarantees from trait.
        unsafe {
            self.initialize(wake_signal);
        }
    }

    #[cfg(debug_assertions)]
    fn inspect_waker_backtraces(&self, _f: &mut dyn FnMut(&std::backtrace::Backtrace)) {}
}
