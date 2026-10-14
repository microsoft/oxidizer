// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

/// Lightweight event-callback subscriber shared by diagnostic tests; spans are ignored.
pub(crate) struct OnEvent<F>(pub(crate) F);

impl<F: Fn() + Send + Sync + 'static> Subscriber for OnEvent<F> {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, _: &Event<'_>) {
        (self.0)();
    }
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}
