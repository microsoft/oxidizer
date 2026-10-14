// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

pub(crate) struct OnEvent<F>(pub(crate) F);

impl<F: Fn() + Send + Sync + 'static> tracing::Subscriber for OnEvent<F> {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, _: &tracing::Event<'_>) {
        (self.0)();
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}
