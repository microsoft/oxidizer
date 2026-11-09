// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::registry::PanicAction;

/// Reports that Arty contained a panic from user task code.
///
/// This event covers async task factories, async task polls, and blocking
/// functions. An async event runs on the task's worker before Arty notifies its
/// join handle. A blocking function runs on a pool thread, so Arty sends its
/// event to the associated async worker before it notifies the join handle.
///
/// The default response is shutdown. Call [`ignore`](Self::ignore) from the
/// callback to keep the runtime available. Call [`shutdown`](Self::shutdown) to
/// select shutdown explicitly. The failed task returns
/// [`JoinError`](crate::task::JoinError) with either response.
///
/// The callback must not panic. Arty cannot repair application data that a
/// task panic left in an invalid state.
#[derive(derive_more::Debug)]
pub struct OnPanic {
    #[debug(ignore)]
    pub(super) action: Arc<std::sync::atomic::AtomicU8>,
}

impl OnPanic {
    /// Keeps the runtime running after this contained panic.
    pub fn ignore(&self) {
        self.action.store(PanicAction::Ignore as u8, Ordering::Release);
    }

    /// Stops the runtime after this contained panic.
    pub fn shutdown(&self) {
        self.action.store(PanicAction::Shutdown as u8, Ordering::Release);
    }
}
