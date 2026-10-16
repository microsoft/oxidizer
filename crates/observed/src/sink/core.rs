// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Core sink type and lifecycle.

use std::any::type_name;
use std::slice;
use std::sync::Arc;

use performables::arc::{Arc as PerformableArc, PerProcess};
use tick::SimpleClock;

use crate::context::Transfer;
use crate::enrichment::{EnrichmentEntry, EnrichmentTransfer, Guard, Slot};
use crate::interop::DynEvent;
use crate::metadata::{EventDescription, SourceLocation};
use crate::processing::{EventProcessor, EventView, IntermediateEvent};
use crate::sampling::{EventSampler, EventSamplingContext, EventSamplingDecision};
use crate::{Event, FlushError, SinkBuilder, SinkFlushError, SinkId};

/// The no-op sink's id returned by [`Sink::noop`]'s `id()` accessor
/// - surfaces in `Debug` output and in error messages.
const NOOP_ID: SinkId = SinkId::new("noop");

/// A sentinel id returned by composite emitters. Composites have no
/// identity of their own - events dispatched through them carry each
/// child's id, not this one. Used only for `Debug` output.
const COMPOSITE_ID: SinkId = SinkId::new("<composite>");

/// Interested processors are tracked in a `u64` bitmask, one bit per
/// processor across every child Sink of a composite.
const MAX_PROCESSORS: usize = u64::BITS as usize;

/// Dispatches events to one or more [`EventProcessor`]s.
///
/// Construct via [`Sink::builder`]. Clone is cheap (`Arc` internals).
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
    #[thread_aware(skip)] // Sink state intentionally stays shared across all threads.
    inner: PerformableArc<SinkInner, PerProcess>,
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
    /// Starts building a [`Sink`].
    ///
    /// `id` accepts a [`SinkId`] or a `&'static str`.
    /// [`enrich_for`](crate::enrichment::EnrichFutureExt::enrich_for) addresses
    /// entries to a sink by this id.
    ///
    /// All other settings are optional; see [`SinkBuilder`].
    ///
    /// # Examples
    ///
    /// ```
    /// use observed::Sink;
    ///
    /// let sink = Sink::builder("service").build();
    /// ```
    #[must_use]
    pub fn builder(id: impl Into<SinkId>) -> SinkBuilder {
        SinkBuilder::new(id.into())
    }

    /// Constructs a sink from the settings collected by [`SinkBuilder`].
    ///
    /// # Panics
    ///
    /// Panics if `processors` holds more than 64 processors.
    pub(crate) fn new(
        id: SinkId,
        processors: Vec<Arc<dyn EventProcessor>>,
        clock: SimpleClock,
        isolated_enrichment: bool,
        sampler: Option<PerformableArc<dyn EventSampler>>,
    ) -> Self {
        assert_within_processor_limit(processors.len());
        Self {
            inner: PerformableArc::from_unaware(SinkInner::Single(SingleSinkState {
                id,
                processors: processors.into_iter().map(PerformableArc::from_std).collect(),
                isolated_enrichment,
                enrichment: Slot::new(),
                clock,
                sampler,
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
    /// at construction time). Each leaf must appear exactly once, and the
    /// leaves together hold at most 64 processors - see [Panics](#panics).
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
    ///
    /// # Panics
    ///
    /// Panics if the same leaf reaches this call twice - directly, via a clone,
    /// or through overlapping nested composites. Cloned leaves share one
    /// enrichment slot, so a duplicate would push onto and later restore the
    /// same thread-local chain twice, stranding a live enrichment node after
    /// the scope ends, and would dispatch every event to those processors more
    /// than once. There is no case where that is what the caller wanted, so it
    /// fails loudly at construction rather than silently at emit time.
    ///
    /// Also panics if the leaves hold more than 64 processors in total.
    #[must_use]
    pub fn composite(children: impl IntoIterator<Item = Self>) -> Self {
        fn push(states: &mut Vec<SingleSinkState>, state: SingleSinkState) {
            assert!(
                !states.iter().any(|known| known.enrichment.ptr_eq(&state.enrichment)),
                "Sink::composite received the same leaf twice (id {}); \
                 remove the duplicate or the overlap between nested composites",
                state.id,
            );
            states.push(state);
        }

        let mut states = Vec::new();
        for child in children {
            match &*child.inner {
                SinkInner::Single(state) => push(&mut states, state.clone()),
                SinkInner::Composite { children } => {
                    for child in children {
                        push(&mut states, child.clone());
                    }
                }
                SinkInner::Noop { .. } => {}
            }
        }
        assert_within_processor_limit(states.iter().map(|state| state.processors.len()).sum());
        // Composites are built once and held for the sink's lifetime; trim any
        // growth slack so the flattened child list doesn't over-retain.
        states.shrink_to_fit();
        Self {
            inner: PerformableArc::from_unaware(SinkInner::Composite { children: states }),
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
            inner: PerformableArc::from_unaware(SinkInner::Noop { enrichment: Slot::new() }),
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
    /// `flush()` returns. Every processor is flushed even after one fails,
    /// and every failure is reported.
    ///
    /// # Errors
    ///
    /// Returns a [`SinkFlushError`] carrying one [`FlushError`]
    /// per processor that failed, in flush order.
    pub fn flush(&self) -> Result<(), SinkFlushError> {
        let failures = match &*self.inner {
            SinkInner::Single(state) => flush_failures(state.processors.iter()),
            SinkInner::Composite { children } => flush_failures(children.iter().flat_map(|c| c.processors.iter())),
            SinkInner::Noop { .. } => Vec::new(),
        };

        if failures.is_empty() {
            Ok(())
        } else {
            Err(SinkFlushError::from_failures(failures))
        }
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
    /// [`EnrichFnExt`](crate::enrichment::EnrichFnExt).
    ///
    /// An empty layer returns early, which spares composite fan-out and every
    /// downstream slot write. The entry slice is built by the caller, so this
    /// does not avoid the slice's own allocation.
    pub(crate) fn push_enrichment(&self, entries: PerformableArc<[EnrichmentEntry]>) -> Guard {
        if entries.is_empty() {
            return Guard::empty();
        }

        match &*self.inner {
            SinkInner::Single(state) => state.enrichment.push(entries),
            SinkInner::Composite { children } => Guard::merge(children.iter().map(|c| c.enrichment.push(PerformableArc::clone(&entries)))),
            SinkInner::Noop { enrichment } => enrichment.push(entries),
        }
    }

    /// Dispatches the event that `build` returns to the processors that want
    /// it, unless an attached [`EventSampler`] drops it. `build` runs only if
    /// at least one processor receives the event.
    ///
    /// Called by the [`emit!`](crate::emit!) macro with the captured
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

        let description = state.description();
        let bitmask = {
            let Some(_guard) = super::try_acquire_reentrancy_guard() else {
                return;
            };
            self.sinks_with_shift().fold(0, |bitmask, (sink, shift)| {
                bitmask | sink.sample_and_ask_processors(&description) << shift
            })
        };
        if bitmask == 0 {
            return;
        }

        let event = state.evaluate();

        let _guard = super::try_acquire_reentrancy_guard().expect("the reentrancy slot should be free again once the event is built");
        for (sink, shift) in self.sinks_with_shift() {
            sink.dispatch(&event, bitmask >> shift);
        }
    }

    fn sinks_with_shift(&self) -> impl Iterator<Item = (&SingleSinkState, usize)> {
        let sinks = match &*self.inner {
            SinkInner::Single(state) => slice::from_ref(state),
            SinkInner::Composite { children } => children.as_slice(),
            SinkInner::Noop { .. } => &[],
        };
        sinks.iter().filter(|sink| !sink.processors.is_empty()).scan(0, |next_shift, sink| {
            let shift = *next_shift;
            *next_shift += sink.processors.len();
            Some((sink, shift))
        })
    }

    /// Returns a `Vec` of all enrichment entries from outermost to innermost scope.
    ///
    /// Returns empty for a [`composite`](Self::composite), which holds no
    /// enrichment of its own - query the leaf sinks instead. This function is
    /// gated behind the `test-util` feature and is intended for testing and
    /// diagnostics only.
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
        let capacity = slices.iter().map(|slice| slice.len()).sum();
        let mut result = Vec::with_capacity(capacity);
        for slice in slices.into_iter().rev() {
            result.extend_from_slice(slice);
        }
        result
    }
}

/// Inner state held behind a [`performables::arc::Arc`]. Each variant carries
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

/// Flushes every processor, collecting all failures rather than stopping at the
/// first, so a partial failure is reported in full.
fn flush_failures<'a>(processors: impl Iterator<Item = &'a PerformableArc<dyn EventProcessor>>) -> Vec<FlushError> {
    processors.filter_map(|p| p.flush().err()).collect()
}

fn assert_within_processor_limit(processor_count: usize) {
    assert!(
        processor_count <= MAX_PROCESSORS,
        "a Sink dispatches to at most {MAX_PROCESSORS} processors, counting every child Sink of a composite, but got {processor_count}",
    );
}

/// The full state of a single leaf sink. Shared (via `Arc`/`Slot` internals)
/// between a `Single` sink and any `Composite` it is folded into, so
/// enrichment pushed on either is visible to the other.
#[derive(Clone)]
struct SingleSinkState {
    id: SinkId,
    processors: PerformableArc<[PerformableArc<dyn EventProcessor>]>,
    isolated_enrichment: bool,
    enrichment: Slot,
    clock: SimpleClock,
    sampler: Option<PerformableArc<dyn EventSampler>>,
}

impl SingleSinkState {
    fn sample_and_ask_processors(&self, description: &EventDescription) -> u64 {
        let context = EventSamplingContext::new(description, &self.processors);
        if let Some(sampler) = &self.sampler
            && sampler.sample(&context) == EventSamplingDecision::Drop
        {
            return 0;
        }
        context.ask_all_processors()
    }

    fn dispatch(&self, event: &dyn DynEvent, bitmask: u64) {
        // Clears the bits of later child Sinks, so no `EventView` is built when none of this Sink's processors want the event.
        let bitmask = bitmask & (u64::MAX >> (u64::BITS as usize - self.processors.len()));
        if bitmask == 0 {
            return;
        }

        let view = EventView::new(
            event,
            self.enrichment.current(),
            self.isolated_enrichment,
            self.id,
            self.clock.system_time(),
        );
        for (index, processor) in self.processors.iter().enumerate() {
            if bitmask & (1 << index) != 0 {
                processor.process(&view);
            }
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::cell::Cell;
    use std::ops::ControlFlow;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use ohno::assert_error_message;

    use super::*;
    use crate::typestates::Set;

    // These atomics are observation counters only. Assertions read them after the
    // synchronous emission, so no ordering relative to other memory is required.

    #[test]
    fn noop_sink_is_noop() {
        assert!(Sink::noop().is_noop());
    }

    struct DummyDyn;

    impl crate::interop::DynEvent for DummyDyn {
        fn name(&self) -> &'static str {
            "dummy"
        }
        fn body(&self) -> Option<Cow<'static, str>> {
            None
        }
        fn source_file(&self) -> Option<Cow<'static, str>> {
            None
        }
        fn source_line(&self) -> Option<u32> {
            None
        }
        fn source_crate(&self) -> Option<Cow<'static, str>> {
            None
        }
        fn visit_fields(&self, _visitor: &mut crate::processing::FieldVisitorFn<'_>) -> ControlFlow<()> {
            ControlFlow::Continue(())
        }
        fn description(&self) -> EventDescription {
            EventDescription::new("dummy", None, None, None, false, false)
        }
    }

    #[test]
    fn current_enrichments_on_composite_is_empty() {
        // A composite carries no enrichment of its own, so the accessor short-circuits.
        let composite = Sink::composite([Sink::noop()]);
        assert!(composite.current_enrichments().is_empty());
    }

    #[test]
    fn noop_sink_dispatch_and_flush() {
        // `emit_impl` gates on `is_noop()`, so these no-op arms are only reachable
        // by calling the pipeline hooks directly.
        let noop = Sink::noop();

        assert_eq!(noop.sinks_with_shift().count(), 0);
        noop.flush().expect("noop flush should succeed");
    }

    /// A processor whose `flush` always fails, used to pin the error-propagation
    /// contract of [`Sink::flush`] and of the blanket `EventProcessor for Arc<T>`.
    struct FailingFlushProcessor;

    impl EventProcessor for FailingFlushProcessor {
        fn is_interested(&self, _description: &EventDescription) -> bool {
            false
        }

        fn process(&self, _event: &EventView<'_>) {}

        fn flush(&self) -> Result<(), FlushError> {
            Err(FlushError::new("failing-flush", "flush boom"))
        }
    }

    fn failing_sink(id: &'static str) -> Sink {
        Sink::builder(id)
            .clock(SimpleClock::new_frozen())
            .processors(vec![Arc::new(FailingFlushProcessor) as Arc<dyn EventProcessor>])
            .build()
    }

    #[test]
    fn standard_processor_pointer_is_wrapped_without_copying() {
        let processor: Arc<dyn EventProcessor> = Arc::new(FailingFlushProcessor);
        let sink = Sink::builder("shared")
            .clock(SimpleClock::new_frozen())
            .processors(vec![Arc::clone(&processor)])
            .build();
        let SinkInner::Single(state) = &*sink.inner else {
            panic!("expected a single sink");
        };

        assert!(std::ptr::eq(Arc::as_ptr(&processor), PerformableArc::as_ptr(&state.processors[0]),));
        assert_eq!(Arc::strong_count(&processor), 2);
        drop(sink);
        assert_eq!(Arc::strong_count(&processor), 1);
    }

    #[test]
    fn single_sink_flush_propagates_processor_error() {
        let err = failing_sink("failing").flush().expect_err("flush must surface the processor error");

        assert_eq!(err.failures().len(), 1);
        assert_eq!(err.failures()[0].processor(), "failing-flush");
        assert_error_message!(
            err,
            "1 processor failed to flush:\n\
             - processor `failing-flush` failed to flush\ncaused by: flush boom"
        );
    }

    #[test]
    fn composite_sink_flush_propagates_first_child_error() {
        let composite = Sink::composite([failing_sink("failing"), Sink::noop()]);
        let err = composite.flush().expect_err("composite flush must surface the child error");

        assert_eq!(err.failures().len(), 1);
        assert_error_message!(
            err,
            "1 processor failed to flush:\n\
             - processor `failing-flush` failed to flush\ncaused by: flush boom"
        );
    }

    /// `Sink::flush` reports every failure, not just the first, so a caller can
    /// tell partial failure from total failure.
    #[test]
    fn composite_sink_flush_reports_every_child_error() {
        let composite = Sink::composite([failing_sink("first"), failing_sink("second")]);
        let err = composite.flush().expect_err("composite flush must surface the child errors");

        assert_eq!(err.failures().len(), 2);
    }

    #[test]
    fn arc_event_processor_flush_delegates_to_inner() {
        let processor: Arc<dyn EventProcessor> = Arc::new(FailingFlushProcessor);
        let err = EventProcessor::flush(&processor).expect_err("Arc must delegate flush to the inner processor");
        assert_error_message!(err, "processor `failing-flush` failed to flush");
    }

    /// A processor that accepts everything, used as the interested peer.
    #[derive(Default)]
    struct AlwaysInterestedProcessor {
        processed: AtomicUsize,
    }

    impl EventProcessor for AlwaysInterestedProcessor {
        fn is_interested(&self, _description: &EventDescription) -> bool {
            true
        }

        fn process(&self, _event: &EventView<'_>) {
            self.processed.fetch_add(1, Ordering::Relaxed);
        }

        fn flush(&self) -> Result<(), FlushError> {
            Ok(())
        }
    }

    struct AlwaysOffSampler;

    impl EventSampler for AlwaysOffSampler {
        fn sample(&self, _context: &EventSamplingContext<'_>) -> EventSamplingDecision {
            EventSamplingDecision::Drop
        }
    }

    /// A processor that never wants anything, used to pin per-processor routing.
    #[derive(Default)]
    struct NeverInterestedProcessor {
        processed: AtomicUsize,
    }

    impl EventProcessor for NeverInterestedProcessor {
        fn is_interested(&self, _description: &EventDescription) -> bool {
            false
        }

        fn process(&self, _event: &EventView<'_>) {
            self.processed.fetch_add(1, Ordering::Relaxed);
        }

        fn flush(&self) -> Result<(), FlushError> {
            Ok(())
        }
    }

    #[test]
    fn always_off_sampler_always_drops() {
        let processor = Arc::new(AlwaysInterestedProcessor::default());
        let sink = Sink::builder("sampled")
            .clock(SimpleClock::new_frozen())
            .processors(vec![Arc::clone(&processor) as Arc<dyn EventProcessor>])
            .event_sampler(AlwaysOffSampler)
            .build();

        crate::interop::emit_dyn_event(&sink, &DummyDyn);

        assert_eq!(processor.processed.load(Ordering::Relaxed), 0);
    }

    /// `is_interested` is the per-processor routing decision, not merely a
    /// collective gate: a processor that declines is skipped even when a peer
    /// on the same sink accepts.
    #[test]
    fn uninterested_processor_is_skipped_despite_interested_peer() {
        let interested = Arc::new(AlwaysInterestedProcessor::default());
        let uninterested = Arc::new(NeverInterestedProcessor::default());
        let sink = Sink::builder("mixed")
            .clock(SimpleClock::new_frozen())
            .processors(vec![
                Arc::clone(&uninterested) as Arc<dyn EventProcessor>,
                Arc::clone(&interested) as Arc<dyn EventProcessor>,
            ])
            .build();

        crate::interop::emit_dyn_event(&sink, &DummyDyn);

        assert_eq!(uninterested.processed.load(Ordering::Relaxed), 0);
        assert_eq!(interested.processed.load(Ordering::Relaxed), 1);
    }

    /// A processor that wants every event or none, counting how often it is
    /// asked and how many events it receives.
    struct CountingProcessor {
        wants: bool,
        asked: AtomicUsize,
        processed: AtomicUsize,
    }

    impl CountingProcessor {
        fn new(wants: bool) -> Arc<Self> {
            Arc::new(Self {
                wants,
                asked: AtomicUsize::new(0),
                processed: AtomicUsize::new(0),
            })
        }

        fn asked(&self) -> usize {
            self.asked.load(Ordering::Relaxed)
        }

        fn processed(&self) -> usize {
            self.processed.load(Ordering::Relaxed)
        }
    }

    impl EventProcessor for CountingProcessor {
        fn is_interested(&self, _description: &EventDescription) -> bool {
            self.asked.fetch_add(1, Ordering::Relaxed);
            self.wants
        }

        fn process(&self, _event: &EventView<'_>) {
            self.processed.fetch_add(1, Ordering::Relaxed);
        }

        fn flush(&self) -> Result<(), FlushError> {
            Ok(())
        }
    }

    /// Returns a fixed decision without asking the processors.
    struct FixedSampler {
        decision: EventSamplingDecision,
        calls: Arc<AtomicUsize>,
    }

    fn fixed_sampler(decision: EventSamplingDecision) -> (FixedSampler, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let sampler = FixedSampler {
            decision,
            calls: Arc::clone(&calls),
        };
        (sampler, calls)
    }

    impl EventSampler for FixedSampler {
        fn sample(&self, _context: &EventSamplingContext<'_>) -> EventSamplingDecision {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.decision
        }
    }

    fn counting_leaf(id: &'static str, processors: &[&Arc<CountingProcessor>]) -> SinkBuilder<Set> {
        Sink::builder(id).clock(SimpleClock::new_frozen()).processors(
            processors
                .iter()
                .map(|processor| Arc::clone(processor) as Arc<dyn EventProcessor>)
                .collect(),
        )
    }

    #[test]
    fn leaf_samplers_in_composite_decide_for_their_own_leaf() {
        let dropped = CountingProcessor::new(true);
        let kept = CountingProcessor::new(true);
        let (drop_sampler, drop_calls) = fixed_sampler(EventSamplingDecision::Drop);
        let (keep_sampler, keep_calls) = fixed_sampler(EventSamplingDecision::Continue);
        let sink = Sink::composite([
            counting_leaf("dropped", &[&dropped]).event_sampler(drop_sampler).build(),
            counting_leaf("kept", &[&kept]).event_sampler(keep_sampler).build(),
        ]);

        crate::interop::emit_dyn_event(&sink, &DummyDyn);

        assert_eq!(drop_calls.load(Ordering::Relaxed), 1);
        assert_eq!(keep_calls.load(Ordering::Relaxed), 1);
        assert_eq!(dropped.processed(), 0);
        assert_eq!(kept.processed(), 1);
    }

    #[test]
    fn leaf_sampler_delivers_beside_processorless_leaf() {
        let processor = CountingProcessor::new(true);
        let (sampler, calls) = fixed_sampler(EventSamplingDecision::Continue);
        let sink = Sink::composite([
            Sink::builder("processorless").clock(SimpleClock::new_frozen()).build(),
            counting_leaf("full", &[&processor]).event_sampler(sampler).build(),
        ]);

        crate::interop::emit_dyn_event(&sink, &DummyDyn);

        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(processor.processed(), 1);
    }

    #[test]
    fn processorless_leaf_in_composite_never_calls_its_sampler() {
        let processor = CountingProcessor::new(true);
        let (sampler, calls) = fixed_sampler(EventSamplingDecision::Continue);
        let sink = Sink::composite([
            Sink::builder("processorless")
                .clock(SimpleClock::new_frozen())
                .event_sampler(sampler)
                .build(),
            counting_leaf("full", &[&processor]).build(),
        ]);

        crate::interop::emit_dyn_event(&sink, &DummyDyn);

        assert_eq!(calls.load(Ordering::Relaxed), 0);
        assert_eq!(processor.processed(), 1);
    }

    #[test]
    fn continue_without_asking_asks_each_processor_once() {
        let processors = [false, true, false, true].map(CountingProcessor::new);
        let (sampler, calls) = fixed_sampler(EventSamplingDecision::Continue);
        let sink = counting_leaf("mixed", &processors.each_ref()).event_sampler(sampler).build();

        crate::interop::emit_dyn_event(&sink, &DummyDyn);

        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(processors.each_ref().map(|processor| processor.asked()), [1, 1, 1, 1]);
        assert_eq!(processors.each_ref().map(|processor| processor.processed()), [0, 1, 0, 1]);
    }

    #[test]
    fn sampler_asking_twice_asks_each_processor_once() {
        struct AskTwiceSampler;

        impl EventSampler for AskTwiceSampler {
            fn sample(&self, context: &EventSamplingContext<'_>) -> EventSamplingDecision {
                assert!(context.processors_interested());
                assert!(context.processors_interested());
                EventSamplingDecision::Continue
            }
        }

        let processors = [false, true, false, true].map(CountingProcessor::new);
        let sink = counting_leaf("mixed", &processors.each_ref())
            .event_sampler(AskTwiceSampler)
            .build();

        crate::interop::emit_dyn_event(&sink, &DummyDyn);

        assert_eq!(processors.each_ref().map(|processor| processor.asked()), [1, 1, 1, 1]);
        assert_eq!(processors.each_ref().map(|processor| processor.processed()), [0, 1, 0, 1]);
    }

    #[test]
    fn continued_event_no_processor_wants_is_never_built() {
        struct ProbeEvent;

        impl Event for ProbeEvent {
            const DESCRIPTION: EventDescription = EventDescription::new("probe", None, None, None, false, false);

            fn visit_fields(&self, _visitor: &mut crate::processing::FieldVisitorFn<'_>) -> ControlFlow<()> {
                ControlFlow::Continue(())
            }
        }

        let uninterested = CountingProcessor::new(false);
        let (sampler, calls) = fixed_sampler(EventSamplingDecision::Continue);
        let sink = counting_leaf("uninterested", &[&uninterested]).event_sampler(sampler).build();
        let built = Cell::new(false);

        sink.emit(
            || {
                built.set(true);
                ProbeEvent
            },
            SourceLocation::new("observed", file!(), line!()),
        );

        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(!built.get());
        assert_eq!(uninterested.processed(), 0);
    }

    #[test]
    #[should_panic(expected = "at most 64 processors")]
    fn build_panics_past_64_processors() {
        let processors: [_; 65] = std::array::from_fn(|_| CountingProcessor::new(true));

        let _sink = counting_leaf("crowded", &processors.each_ref()).build();
    }

    #[test]
    #[should_panic(expected = "at most 64 processors")]
    fn composite_panics_past_64_processors_across_leaves() {
        let first: [_; 32] = std::array::from_fn(|_| CountingProcessor::new(true));
        let second: [_; 33] = std::array::from_fn(|_| CountingProcessor::new(true));

        let _sink = Sink::composite([
            counting_leaf("first", &first.each_ref()).build(),
            counting_leaf("second", &second.each_ref()).build(),
        ]);
    }

    #[test]
    fn last_of_64_processors_receives_event() {
        let processors: [_; 64] = std::array::from_fn(|index| CountingProcessor::new(index == 63));
        let sink = counting_leaf("full", &processors.each_ref()).build();

        crate::interop::emit_dyn_event(&sink, &DummyDyn);

        let expected: [usize; 64] = std::array::from_fn(|index| usize::from(index == 63));
        assert_eq!(processors.each_ref().map(|processor| processor.processed()), expected);
    }

    #[test]
    fn composite_delivers_to_interested_processors_of_each_leaf() {
        let first = [false, true].map(CountingProcessor::new);
        let second = [true, false].map(CountingProcessor::new);
        let sink = Sink::composite([
            counting_leaf("first", &first.each_ref()).build(),
            counting_leaf("second", &second.each_ref()).build(),
        ]);

        crate::interop::emit_dyn_event(&sink, &DummyDyn);

        assert_eq!(first.each_ref().map(|processor| processor.processed()), [0, 1]);
        assert_eq!(second.each_ref().map(|processor| processor.processed()), [1, 0]);
    }

    #[test]
    fn composite_of_64_processors_delivers_to_last_processor() {
        let declining: [_; 63] = std::array::from_fn(|_| CountingProcessor::new(false));
        let accepting = CountingProcessor::new(true);
        let sink = Sink::composite([
            counting_leaf("declining", &declining.each_ref()).build(),
            counting_leaf("accepting", &[&accepting]).build(),
            Sink::builder("processorless").clock(SimpleClock::new_frozen()).build(),
        ]);

        crate::interop::emit_dyn_event(&sink, &DummyDyn);

        assert_eq!(declining.each_ref().map(|processor| processor.processed()), [0; 63]);
        assert_eq!(accepting.processed(), 1);
    }

    #[test]
    fn current_enrichments_reports_the_pushed_entries() {
        let sink = Sink::noop();
        let _guard = sink.push_enrichment(PerformableArc::from(vec![
            EnrichmentEntry::unclassified("outer", 1_i64),
            EnrichmentEntry::unclassified("inner", 2_i64),
        ]));

        let entries = sink.current_enrichments();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].key().as_str(), "outer");
        assert_eq!(entries[1].key().as_str(), "inner");
    }
}
