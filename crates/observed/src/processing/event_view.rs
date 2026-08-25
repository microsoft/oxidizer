// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Lazy pull-based event representation for processor consumption.
//!
//! An [`EventView`] wraps a `&dyn DynEvent` together with a snapshot of the
//! enrichment context active at emit time. Processors pull only the data
//! they need - fields that are skipped never invoke their redaction
//! closure, achieving zero cost for rejected fields.
//!
//! The view does **not** retain a reference to the [`Sink`](crate::Sink) that produced
//! it. The enrichment chain head is captured as a cheap [`Arc`] snapshot
//! when the view is constructed, which makes [`EventView`] usable from
//! synthetic / replay paths (e.g. buffered or delayed processors)
//! that do not have a live sink to lend out.

use std::borrow::Cow;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::SystemTime;

use crate::enrichment::{EnrichmentNode, OptEnrichmentNode};
use crate::interop::DynEvent;
use crate::metadata::{EventDescription, FieldDescriptor, LogDescription, LogFieldEntry, MetricFieldEntry};
use crate::processing::FieldValueFn;
use crate::{Severity, SinkId, Value};

/// Snapshot of the enrichment context attached to an event.
///
/// Captured at view-construction time so processors no longer need a live
/// reference to the sink. The [`Arc`] in `head` is the same one held by
/// the sink's thread-local slot; cloning it is one atomic increment.
///
/// `EventEnrichment::empty()` produces a view that yields no enrichment
/// entries - used for synthetic events (e.g. buffered replay).
#[derive(Clone)]
struct EventEnrichment {
    head: OptEnrichmentNode,
    isolated: bool,
    id: SinkId,
}

impl EventEnrichment {
    /// An empty enrichment context - `visit` yields nothing.
    pub(crate) fn empty() -> Self {
        // `key` / `isolated` are unused when `head` is `None`.
        const PLACEHOLDER: SinkId = SinkId::new("<replay>");
        Self {
            head: None,
            isolated: false,
            id: PLACEHOLDER,
        }
    }

    /// Walks the enrichment chain outermost-first, applies isolation and
    /// targeting filters, and invokes `visitor` for each surviving entry.
    fn visit(&self, visitor: &mut dyn FnMut(&FieldDescriptor, &FieldValueFn<'_>) -> ControlFlow<()>) -> ControlFlow<()> {
        // Enrichment chains are typically short; reserve stack space to avoid heap alloc in the common case.
        const EXPECTED_NODES_COUNT: usize = 16;
        if self.head.is_none() {
            return ControlFlow::Continue(());
        }

        // Collect node references so we can iterate outermost-first.
        let mut nodes: smallvec::SmallVec<[&Arc<EnrichmentNode>; EXPECTED_NODES_COUNT]> = smallvec::SmallVec::new();
        let mut cursor = self.head.as_ref();
        while let Some(node) = cursor {
            nodes.push(node);
            cursor = node.parent.as_ref();
        }

        for node in nodes.into_iter().rev() {
            for entry in node.entries.iter() {
                // Apply isolation / targeting rules.
                match entry.target() {
                    Some(target) if target != self.id => continue,
                    None if self.isolated => continue,
                    _ => {}
                }

                let desc = FieldDescriptor::new(
                    entry.key().as_str(),
                    (!entry.is_excluded_from_logs()).then(|| LogFieldEntry::new(entry.key().as_str())),
                    entry.metric_key().map(|key| MetricFieldEntry::dimension(key.as_str())),
                );
                let getter = |redactor: &dyn data_privacy::Redactor| -> Value { entry.redacted_value_inner(redactor) };
                visitor(&desc, &getter)?;
            }
        }
        ControlFlow::Continue(())
    }
}

/// A lazy, pull-based view of an event and its enrichments.
///
/// Processors receive this from [`EventProcessor::process`](super::EventProcessor::process)
/// and pull only the fields/enrichments they need. Skipped fields never
/// invoke their redaction closure.
///
/// The enrichment context is captured as a snapshot at view construction.
/// Concurrent enrichment pushes after that point are not visible to this
/// view.
pub struct EventView<'a> {
    event: &'a dyn DynEvent,
    enrichments: EventEnrichment,
    timestamp: SystemTime,
}

impl<'a> EventView<'a> {
    /// Builds a live event view from an event, the enrichment state of the
    /// sink that produced it, and the `timestamp` stamped by the sink's
    /// [`SimpleClock`](tick::SimpleClock).
    ///
    /// `enrichment_head` is the snapshotted chain head (one atomic `Arc`
    /// clone), `isolated` mirrors the sink's isolation flag, and `id` is the
    /// sink's identity used for targeted-enrichment filtering. The caller
    /// supplies the timestamp (read from the sink's clock) so the view never
    /// calls `SystemTime::now()` directly — keeping it deterministic under a
    /// frozen clock and usable under Miri isolation.
    pub(crate) fn new(
        event: &'a dyn DynEvent,
        enrichment_head: OptEnrichmentNode,
        isolated: bool,
        id: SinkId,
        timestamp: SystemTime,
    ) -> Self {
        Self {
            event,
            enrichments: EventEnrichment {
                head: enrichment_head,
                isolated,
                id,
            },
            timestamp,
        }
    }

    /// Builds a view for a synthetic / replayed event, using the caller-supplied
    /// `timestamp` instead of one captured from a sink clock.
    ///
    /// The view carries **no enrichment**: [`visit_enrichments`](Self::visit_enrichments)
    /// never invokes the visitor. A replayed event must therefore carry any
    /// context it needs as its own fields.
    pub fn new_synthetic(event: &'a dyn DynEvent, timestamp: SystemTime) -> Self {
        Self {
            event,
            enrichments: EventEnrichment::empty(),
            timestamp,
        }
    }

    /// Returns the wall-clock timestamp captured when this view was created.
    ///
    /// All processors sharing this view see the same instant, avoiding
    /// per-destination timestamp drift.
    #[must_use]
    #[inline]
    pub fn timestamp(&self) -> SystemTime {
        self.timestamp
    }

    /// Returns the event name.
    #[must_use]
    #[inline]
    pub fn name(&self) -> &'static str {
        self.event.name()
    }

    /// Returns the event severity (only meaningful for log-producing events).
    ///
    /// Derived from the log signal on [`description`](Self::description), which
    /// is the single statement that decides both routing and the exported
    /// level - an event cannot be routed as a log at one severity and exported
    /// at another.
    #[must_use]
    #[inline]
    pub fn severity(&self) -> Option<Severity> {
        self.event.description().log().map(LogDescription::severity)
    }

    /// Returns the event body (human-readable message), if any.
    ///
    /// The body is not redacted. For a typed event it is a `&'static str`
    /// message template; for a [`DynEvent`] it is trusted, pre-sanitized text
    /// the adaptor is responsible for - see [`DynEvent::body`]. Processors may
    /// export it verbatim; untrusted runtime values belong in fields, which do
    /// pass through the redaction engine.
    #[must_use]
    #[inline]
    pub fn body(&self) -> Option<Cow<'static, str>> {
        self.event.body()
    }

    /// Returns the source file path, if available.
    #[must_use]
    #[inline]
    pub fn source_file(&self) -> Option<Cow<'static, str>> {
        self.event.source_file()
    }

    /// Returns the source line number, if available.
    #[must_use]
    #[inline]
    pub fn source_line(&self) -> Option<u32> {
        self.event.source_line()
    }

    /// Returns the name of the crate where the event originated, if available.
    #[must_use]
    #[inline]
    pub fn source_crate(&self) -> Option<Cow<'static, str>> {
        self.event.source_crate()
    }

    /// Returns the compile-time event description (signals, metrics, etc.).
    #[must_use]
    #[inline]
    pub fn description(&self) -> EventDescription {
        self.event.description()
    }

    /// Visits event fields lazily.
    ///
    /// For each field, the visitor receives a [`FieldDescriptor`] and a getter
    /// closure. The getter is only invoked if the processor wants the value -
    /// it takes a `&dyn Redactor` and returns the redacted [`Value`].
    ///
    /// The visitor returns [`ControlFlow::Continue`] to keep iterating or
    /// [`ControlFlow::Break`] to stop early.
    pub fn visit_fields<V>(&self, visitor: &mut V) -> ControlFlow<()>
    where
        V: FnMut(&FieldDescriptor, &FieldValueFn<'_>) -> ControlFlow<()>,
    {
        self.event.visit_fields(visitor)
    }

    /// Visits enrichment entries lazily.
    ///
    /// Same pattern as [`visit_fields`](Self::visit_fields) - the getter
    /// closure is only invoked if the processor wants the enrichment value.
    ///
    /// Entries are yielded outermost-first. Isolation and targeting rules
    /// captured at view-construction time are applied inline. If duplicate
    /// keys exist the processor sees all of them and decides precedence.
    pub fn visit_enrichments<V>(&self, visitor: &mut V) -> ControlFlow<()>
    where
        V: FnMut(&FieldDescriptor, &FieldValueFn<'_>) -> ControlFlow<()>,
    {
        self.enrichments.visit(&mut |d, g| visitor(d, g))
    }
}

impl std::fmt::Debug for EventView<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("name", &self.event.name())
            .field("severity", &self.severity())
            .finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::processing::FieldVisitorFn;

    /// An event that reports a distinct value from every accessor, so a view
    /// that answers from the wrong one (or from a constant) is visible.
    struct SourcedEvent;

    impl DynEvent for SourcedEvent {
        fn name(&self) -> &'static str {
            "test.event"
        }

        fn body(&self) -> Option<Cow<'static, str>> {
            Some(Cow::Borrowed("a message"))
        }

        fn source_file(&self) -> Option<Cow<'static, str>> {
            Some(Cow::Borrowed("crates/observed/src/lib.rs"))
        }

        fn source_line(&self) -> Option<u32> {
            Some(42)
        }

        fn source_crate(&self) -> Option<Cow<'static, str>> {
            Some(Cow::Borrowed("observed"))
        }

        fn visit_fields(&self, _visitor: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
            ControlFlow::Continue(())
        }

        fn description(&self) -> EventDescription {
            EventDescription::new("test.event", None, None, None, false, false)
        }
    }

    #[test]
    fn view_forwards_the_event_identity_and_source_location() {
        let event = SourcedEvent;
        let view = EventView::new_synthetic(&event, SystemTime::UNIX_EPOCH);

        assert_eq!(view.name(), "test.event");
        assert_eq!(view.source_file(), Some(Cow::Borrowed("crates/observed/src/lib.rs")));
        assert_eq!(view.source_line(), Some(42));
        assert_eq!(view.source_crate(), Some(Cow::Borrowed("observed")));
        assert_eq!(view.body(), Some(Cow::Borrowed("a message")));
    }
}
