// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Core sink type and lifecycle.

use std::any::type_name;
use std::sync::Arc;

use tick::SimpleClock;

use crate::context::Transfer;
use crate::enrichment::{EnrichmentEntry, EnrichmentTransfer, Guard, Slot};
use crate::interop::DynEvent;
use crate::metadata::{EventDescription, SourceLocation};
use crate::processing::{EventProcessor, EventView, IntermediateEvent};
use crate::{Event, SinkId};

/// The no-op sink's id returned by [`Sink::noop`]'s `id()` accessor
/// - surfaces in `Debug` output and in error messages.
const NOOP_ID: SinkId = SinkId::new("noop");

/// A sentinel id returned by composite emitters. Composites have no
/// identity of their own - events dispatched through them carry each
/// child's id, not this one. Used only for `Debug` output.
const COMPOSITE_ID: SinkId = SinkId::new("<composite>");

/// Dispatches events to one or more [`EventProcessor`]s.
///
/// Construct via [`Sink::new`]. Clone is cheap (`Arc` internals).
///
/// Sinks can be combined with [`Sink::composite`] so a single [`emit!`](crate::emit!)
/// fans out through several underlying emitters. Each sink owns its own
/// enrichment slot - enrichments pushed on one sink are **not** visible
/// on another.
///
/// See the [Enrichment](crate#enrichment) section for how scoped context
/// is attached to events via this sink, and the [Quick Start](crate#quick-start)
/// for a usage example.
#[derive(Clone, thread_aware::ThreadAware)]
pub struct Sink {
    inner: thread_aware::Arc<SinkInner, thread_aware::PerProcess>,
}

impl AsRef<Self> for Sink {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl std::fmt::Debug for Sink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &*self.inner {
            SinkInner::Single(state) => f
                .debug_struct(type_name::<Self>())
                .field("variant", &"Single")
                .field("id", &state.id)
                .field("processors", &state.processors.len())
                .field("isolated_enrichment", &state.isolated_enrichment)
                .finish(),
            SinkInner::Composite { children } => f
                .debug_struct(type_name::<Self>())
                .field("variant", &"Composite")
                .field("children", &children.len())
                .finish(),
            SinkInner::Noop { .. } => f.debug_struct(type_name::<Self>()).field("variant", &"Noop").finish(),
        }
    }
}

impl Sink {
    /// Constructs a sink with the given [`SinkId`], processor list, and clock.
    ///
    /// `id` accepts an [`SinkId`] or a `&'static str`; it is the identity
    /// token targeted by `enrich_for(ID, …)`.
    ///
    /// `clock` stamps the timestamp on every event this sink dispatches. It
    /// accepts anything that is `AsRef<SimpleClock>` - both
    /// [`tick::SimpleClock`] and [`tick::Clock`]. In production pass the
    /// application's clock (e.g. `SimpleClock::new_system()`); in tests pass a
    /// frozen clock (`SimpleClock::new_frozen()`) for deterministic, Miri-safe
    /// timestamps.
    ///
    /// The sink receives both untargeted (global) and targeted enrichments.
    /// For a sink that ignores untargeted entries (the library-isolation
    /// pattern), use [`Sink::new_isolated`].
    ///
    /// This is the fundamental primitive: supply the [`EventProcessor`]s that
    /// the sink fans each event out to.
    #[must_use]
    pub fn new(id: impl Into<SinkId>, processors: Vec<Arc<dyn EventProcessor>>, clock: impl AsRef<SimpleClock>) -> Self {
        Self::build_single(id.into(), false, processors, clock)
    }

    /// Like [`Sink::new`], but configures the sink to ignore untargeted
    /// (global) enrichments. Only entries explicitly targeted at this
    /// sink's id (via `enrich_for(ID, …)`) are visible.
    ///
    /// Useful for library emitters that must not inherit application-level
    /// context.
    #[must_use]
    pub fn new_isolated(id: impl Into<SinkId>, processors: Vec<Arc<dyn EventProcessor>>, clock: impl AsRef<SimpleClock>) -> Self {
        Self::build_single(id.into(), true, processors, clock)
    }

    fn build_single(
        id: SinkId,
        isolated_enrichment: bool,
        processors: Vec<Arc<dyn EventProcessor>>,
        clock: impl AsRef<SimpleClock>,
    ) -> Self {
        Self {
            inner: thread_aware::Arc::from_unaware(SinkInner::Single(SingleSinkState {
                id,
                processors: processors.into(),
                isolated_enrichment,
                enrichment: Slot::new(),
                clock: clock.as_ref().clone(),
            })),
        }
    }

    /// Creates a **composite** sink that dispatches every event through
    /// each of `children` in turn.
    ///
    /// A composite is a dispatcher, not an identity. Records shipped during
    /// emission travel through each child's own processors and carry each
    /// child's own [`SinkId`], resource, redaction, and enrichment. The
    /// composite itself owns no providers, holds no enrichment, and is only
    /// used to route one `emit!` call across multiple underlying emitters.
    ///
    /// The resulting sink is an ordinary [`Sink`], so it can be passed
    /// wherever `&Sink` is expected, cloned cheaply, or itself included
    /// in another composite (nesting flattens into a single list of leaves
    /// at construction time).
    ///
    /// # Flush
    ///
    /// Calling [`Sink::flush`] on the composite propagates to every child.
    ///
    /// # Enrichment
    ///
    /// [`.enrich(&composite, ...)`](crate::enrichment::EnrichFutureExt) **broadcasts** the push to every
    /// child's enrichment slot - entries pushed via the composite are visible on
    /// records dispatched to each child. The returned guard pops from each
    /// child's slot when dropped. Each child still has its own slot;
    /// enrichments pushed directly on a child remain visible only on that
    /// child.
    #[must_use]
    pub fn composite(children: impl IntoIterator<Item = Self>) -> Self {
        let mut states = Vec::new();
        for child in children {
            match &*child.inner {
                SinkInner::Single(state) => states.push(state.clone()),
                SinkInner::Composite { children } => states.extend(children.iter().cloned()),
                SinkInner::Noop { .. } => {}
            }
        }
        Self {
            inner: thread_aware::Arc::from_unaware(SinkInner::Composite { children: states }),
        }
    }

    /// Creates a no-op sink that silently drops all events.
    ///
    /// A dedicated `noop()` sink carrying only an enrichment slot - useful as
    /// a test fixture for `.enrich()` semantics - it just doesn't dispatch
    /// anywhere when `emit!` fires.
    // TODO: consider dropping enrichment slot for no-op sinks
    #[must_use]
    pub fn noop() -> Self {
        Self {
            inner: thread_aware::Arc::from_unaware(SinkInner::Noop { enrichment: Slot::new() }),
        }
    }

    /// Returns the [`SinkId`] this sink is bound to.
    ///
    /// Returns a sentinel value (`"<composite>"`) for composite emitters.
    /// `Sink::noop()` returns the `"noop"` sentinel.
    #[must_use]
    pub fn id(&self) -> SinkId {
        match &*self.inner {
            SinkInner::Single(state) => state.id,
            SinkInner::Composite { .. } => COMPOSITE_ID,
            SinkInner::Noop { .. } => NOOP_ID,
        }
    }

    /// Returns `true` if this sink would silently drop all events.
    ///
    /// For a Single sink, true iff it has no processors. For a Composite,
    /// true iff every child is a noop.
    #[must_use]
    pub fn is_noop(&self) -> bool {
        match &*self.inner {
            SinkInner::Single(state) => state.processors.is_empty(),
            SinkInner::Composite { children } => children.iter().all(|c| c.processors.is_empty()),
            SinkInner::Noop { .. } => true,
        }
    }

    /// Forces buffered telemetry out by calling
    /// [`EventProcessor::flush`] on every registered processor (and, for
    /// a [`composite`](Self::composite), recursively on each child's
    /// processors).
    ///
    /// Non-terminating - the sink remains fully usable after
    /// `flush()` returns. Returns the first error encountered; remaining
    /// processors are still flushed.
    ///
    /// # Errors
    ///
    /// Returns the first error reported by any registered processor while
    /// flushing its buffered telemetry.
    pub fn flush(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut first_err: Option<Box<dyn std::error::Error + Send + Sync>> = None;
        match &*self.inner {
            SinkInner::Single(state) => state.flush_into(&mut first_err),
            SinkInner::Composite { children } => {
                for c in children {
                    c.flush_into(&mut first_err);
                }
            }
            SinkInner::Noop { .. } => {}
        }

        first_err.map_or(Ok(()), Err)
    }

    /// Captures the current enrichment context for transfer to another thread.
    ///
    /// For a [`composite`](Self::composite), every child's enrichment is
    /// captured independently, so each child's context round-trips with its
    /// own chain - including enrichments pushed directly on a child rather
    /// than broadcast through the composite.
    pub fn transfer_context(&self) -> Transfer {
        let mut transfer = EnrichmentTransfer::default();
        match &*self.inner {
            SinkInner::Single(state) => transfer.add_slot(&state.enrichment),
            SinkInner::Noop { enrichment } => transfer.add_slot(enrichment),
            SinkInner::Composite { children } => {
                for child in children {
                    transfer.add_slot(&child.enrichment);
                }
            }
        }
        Transfer::new(transfer)
    }

    /// Pushes enrichment entries onto this sink's enrichment chain(s).
    ///
    /// For a Single sink, pushes onto its own slot. For a Composite,
    /// **broadcasts** the push to every child's slot (recursively, so a
    /// composite-of-composites flattens), and returns a compound guard that
    /// pops all children on Drop.
    ///
    /// This is the entry point used by the `.enrich(&sink, ...)` API in
    /// [`EnrichFutureExt`](crate::enrichment::EnrichFutureExt) and
    /// [`EnrichFnExt`](crate::enrichment::EnrichFnExt). Composites with zero children
    /// return a no-op guard.
    pub(crate) fn push_enrichment(&self, entries: Arc<[EnrichmentEntry]>) -> Guard {
        match &*self.inner {
            SinkInner::Single(state) => state.enrichment.push(entries),
            SinkInner::Composite { children } => Guard::merge(children.iter().map(|c| c.enrichment.push(Arc::clone(&entries)))),
            SinkInner::Noop { enrichment } => enrichment.push(entries),
        }
    }

    /// Builds an event via `build` and emits it to every registered processor.
    ///
    /// Called by the [`emit!`](crate::emit) macro with the captured
    /// [`SourceLocation`]; prefer that macro over calling this directly.
    pub fn emit<E: Event, F: FnOnce() -> E>(&self, build: F, source_location: SourceLocation) {
        let state = IntermediateEvent::typed(build, source_location);
        self.emit_impl(state);
    }

    /// Dispatches an event through the sink.
    ///
    /// It's automatically called by the `emit!` macro expansion, and can be called directly for
    /// [`DynEvent`s](DynEvent)
    pub(crate) fn emit_impl<'a, T: Event, F: FnOnce() -> T + 'a>(&self, state: IntermediateEvent<'a, F>) {
        if self.is_noop() {
            return;
        }

        let Some(_guard) = super::try_acquire_reentrancy_guard() else {
            return;
        };

        let description = state.description();
        if !self.is_interested_in(&description) {
            return;
        }

        let event = state.evaluate();
        self.dispatch_to_processors(&event, &description);
    }

    /// Returns `true` if at least one processor is interested in the event.
    ///
    /// For Composite, returns `true` if any child is interested.
    fn is_interested_in(&self, description: &EventDescription) -> bool {
        match &*self.inner {
            SinkInner::Single(state) => state.is_interested(description),
            SinkInner::Composite { children } => children.iter().any(|c| c.is_interested(description)),
            SinkInner::Noop { .. } => false,
        }
    }

    /// Dispatches a `DynEvent` to interested processors - or, for a Composite
    /// sink, delegates dispatch to each child leaf so every leaf constructs its
    /// own `EventView` rooted at itself (which walks its own enrichment slot).
    ///
    /// The reentrancy guard acquired in [`Sink::emit`] is held across all
    /// sibling dispatches, so composites safely iterate children without the
    /// guard falsely tripping.
    fn dispatch_to_processors(&self, event: &dyn DynEvent, description: &EventDescription) {
        match &*self.inner {
            SinkInner::Single(state) => state.dispatch(event, description),
            SinkInner::Composite { children } => {
                for child in children {
                    if child.is_interested(description) {
                        child.dispatch(event, description);
                    }
                }
            }
            SinkInner::Noop { .. } => {}
        }
    }

    /// Returns a `Vec` of all enrichment entries from outermost to innermost scope.
    ///
    /// This function is gated behind the `test-util` feature and is intended for
    /// testing and diagnostics only.
    #[cfg(any(test, feature = "test-util"))]
    #[must_use]
    pub fn current_enrichments(&self) -> Vec<EnrichmentEntry> {
        let slot = match &*self.inner {
            SinkInner::Single(state) => &state.enrichment,
            // For a composite, return an empty vector since it has no enrichment of its own.
            SinkInner::Composite { .. } => {
                return Vec::new();
            }
            SinkInner::Noop { enrichment } => enrichment,
        };

        let head = slot.current();
        // Collect all entries unfiltered for test visibility.
        let mut slices = Vec::new();
        let mut cursor = head.as_ref();
        while let Some(node) = cursor {
            slices.push(&*node.entries);
            cursor = node.parent.as_ref();
        }
        let mut result = Vec::new();
        for slice in slices.into_iter().rev() {
            result.extend_from_slice(slice);
        }
        result
    }
}

/// Inner state held behind a [`thread_aware::Arc`]. Each variant carries
/// only the fields that variant needs - Single carries one leaf's full
/// state; Composite carries a flattened list of leaf states (built at
/// construction time); Noop carries only an enrichment slot.
///
/// `Sink::noop()` is the dedicated `Noop` variant: it owns no processors
/// and never dispatches, but keeps a working enrichment slot so noop sinks
/// remain useful as enrichment-API test fixtures.
#[derive(Clone)]
enum SinkInner {
    /// A leaf sink with its own processors, id, and enrichment slot.
    /// Processors are `Arc`-shared so they may carry their own owned
    /// resources (e.g. `OTel` providers); their `Drop` fires when the last
    /// `Sink` clone is released.
    Single(SingleSinkState),
    /// Routes one `emit!` call through each leaf in turn. Children are the
    /// flattened leaf states of every sink passed to [`Sink::composite`],
    /// each sharing its enrichment slot (`Arc`-backed) with the originating
    /// sink.
    Composite { children: Vec<SingleSinkState> },
    /// A sink that drops every event. Carries only an enrichment slot so it
    /// can still be used as an `.enrich()` test fixture.
    Noop { enrichment: Slot },
}

/// The full state of a single leaf sink. Shared (via `Arc`/`Slot` internals)
/// between a `Single` sink and any `Composite` it is folded into, so
/// enrichment pushed on either is visible to the other.
#[derive(Clone)]
struct SingleSinkState {
    id: SinkId,
    processors: Arc<[Arc<dyn EventProcessor>]>,
    isolated_enrichment: bool,
    enrichment: Slot,
    clock: SimpleClock,
}

impl SingleSinkState {
    /// Returns `true` if any of this leaf's processors is interested in the event.
    fn is_interested(&self, description: &EventDescription) -> bool {
        self.processors.iter().any(|p| p.is_interested(description))
    }

    /// Flushes every processor, recording the first error (if any) into `first_err`.
    fn flush_into(&self, first_err: &mut Option<Box<dyn std::error::Error + Send + Sync>>) {
        for p in self.processors.iter() {
            if let Err(e) = p.flush() {
                first_err.get_or_insert(e);
            }
        }
    }

    /// Builds an [`EventView`] rooted at this leaf's enrichment slot and
    /// hands it to every interested processor.
    fn dispatch(&self, event: &dyn DynEvent, description: &EventDescription) {
        // Reading the leaf's clock keeps timestamps off `SystemTime::now()`,
        // so frozen clocks make this Miri-safe.
        let timestamp = self.clock.system_time();
        let view = EventView::new(event, self.enrichment.current(), self.isolated_enrichment, self.id, timestamp);
        for processor in self.processors.iter() {
            if processor.is_interested(description) {
                processor.process(&view);
            }
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_sink_is_noop() {
        assert!(Sink::noop().is_noop());
    }
}
