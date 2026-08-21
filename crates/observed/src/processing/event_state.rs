// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Event state machine for lazy and dynamic dispatch.

use std::borrow::Cow;
use std::ops::ControlFlow;

use crate::Event;
use crate::interop::DynEvent;
use crate::metadata::{EventDescription, SourceLocation};
use crate::processing::FieldVisitorFn;

/// An intermediate event state that can be either a lazy typed event or a pre-constructed dynamic
/// event reference.
pub(crate) struct IntermediateEvent<'a, F> {
    inner: Inner<'a, F>,
}

impl<T: Event, F: FnOnce() -> T> IntermediateEvent<'_, F> {
    pub(crate) fn typed(build: F, source_location: SourceLocation) -> Self {
        Self {
            inner: Inner::NotEvaluated(build, source_location),
        }
    }
}

impl<'a> IntermediateEvent<'a, fn() -> NoopEvent> {
    /// Wraps a pre-constructed dynamic event reference for direct dispatch.
    ///
    /// Used by the type-erased
    /// [`emit_dyn_event`](crate::interop::emit_dyn_event) entry point.
    pub(crate) fn dynamic(event: &'a dyn DynEvent) -> Self {
        Self { inner: Inner::Dyn(event) }
    }
}

impl<'a, T: Event, F: FnOnce() -> T> IntermediateEvent<'a, F> {
    /// Returns the event description for the pre-construction interest check.
    ///
    /// For typed variants this is the static `T::DESCRIPTION` (no allocation).
    /// For dynamic variants this delegates to `DynEvent::description()`.
    pub(crate) fn description(&self) -> EventDescription {
        match self.inner {
            Inner::NotEvaluated(..) => T::DESCRIPTION,
            Inner::Dyn(event) => event.description(),
        }
    }

    /// Evaluates the event (if lazy) and returns an [`EvaluatedEvent`] that
    /// implements [`DynEvent`] and can be passed directly to processors.
    pub(crate) fn evaluate(self) -> EvaluatedEvent<'a, T> {
        match self.inner {
            Inner::NotEvaluated(build, source_location) => EvaluatedEvent::Typed(build(), source_location),
            Inner::Dyn(event) => EvaluatedEvent::Dyn(event),
        }
    }
}

/// Describes the state of an event being emitted.
///
/// This enum unifies lazy and eager, typed and dynamic event dispatch behind
/// a single [`Sink::emit`](crate::Sink::emit) entry point.
///
/// - **Typed** variant (`NotEvaluated`) carries a concrete `T: Event`
///   and uses `T::DESCRIPTION` for the pre-construction interest check.
/// - **Dynamic** variant (`Dyn`) uses `NoopEvent` as the `T` parameter and
///   obtains the description from the `&dyn DynEvent` at runtime.
enum Inner<'a, F> {
    /// Lazy typed event - transitions to `ReadyEvent::Evaluated` via
    /// [`evaluate`](IntermediateEvent::evaluate) only if at least one processor is interested in
    /// `T::DESCRIPTION`.
    NotEvaluated(F, SourceLocation),
    /// Pre-constructed dynamic event reference.
    Dyn(&'a dyn DynEvent),
}

/// An evaluated event ready for dispatch, implementing [`DynEvent`].
pub(crate) enum EvaluatedEvent<'a, T: Event> {
    Typed(T, SourceLocation),
    Dyn(&'a dyn DynEvent),
}

impl<T: Event> DynEvent for EvaluatedEvent<'_, T> {
    fn name(&self) -> &'static str {
        match self {
            Self::Typed(_, _) => T::DESCRIPTION.name(),
            Self::Dyn(event) => event.name(),
        }
    }

    fn body(&self) -> Option<Cow<'static, str>> {
        match self {
            Self::Typed(_, _) => T::DESCRIPTION
                .log()
                .and_then(super::super::metadata::LogDescription::body)
                .map(Cow::Borrowed),
            Self::Dyn(event) => event.body(),
        }
    }

    fn source_file(&self) -> Option<Cow<'static, str>> {
        match self {
            Self::Typed(_, sl) => Some(Cow::Borrowed(sl.file())),
            Self::Dyn(event) => event.source_file(),
        }
    }

    fn source_line(&self) -> Option<u32> {
        match self {
            Self::Typed(_, sl) => Some(sl.line()),
            Self::Dyn(event) => event.source_line(),
        }
    }

    fn source_crate(&self) -> Option<Cow<'static, str>> {
        match self {
            Self::Typed(_, sl) => Some(Cow::Borrowed(sl.crate_name())),
            Self::Dyn(event) => event.source_crate(),
        }
    }

    fn visit_fields(&self, visitor: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
        match self {
            Self::Typed(event, _) => event.visit_fields(visitor),
            Self::Dyn(event) => event.visit_fields(visitor),
        }
    }

    fn description(&self) -> EventDescription {
        match self {
            Self::Typed(_, _) => T::DESCRIPTION,
            Self::Dyn(event) => event.description(),
        }
    }
}

/// A no-op event used as the type parameter `T` in [`IntermediateEvent`]
/// for dynamic event variants.
///
/// This type is never constructed at runtime; it exists solely to satisfy
/// the `T: Event` bound when the actual event is dispatched via `&dyn DynEvent`.
pub(crate) struct NoopEvent;

impl Event for NoopEvent {
    const DESCRIPTION: EventDescription = EventDescription::new("noop", None, None, None, false, false);

    #[cfg_attr(coverage_nightly, coverage(off))]
    fn visit_fields(&self, _visitor: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
        unreachable!("NoopEvent should never have its fields accessed")
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    struct TestEvent;

    impl Event for TestEvent {
        const DESCRIPTION: EventDescription = EventDescription::new("test.event", None, None, None, false, false);

        fn visit_fields(&self, _visitor: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
            ControlFlow::Continue(())
        }
    }

    #[test]
    fn a_typed_event_reports_its_name_and_captured_source_location() {
        // `emit!` captures the call site and the evaluated event is what carries
        // it to a processor, so each accessor must report its own part of it.
        let evaluated = EvaluatedEvent::Typed(TestEvent, SourceLocation::new("observed", "crates/observed/src/lib.rs", 42));

        assert_eq!(evaluated.name(), "test.event");
        assert_eq!(evaluated.source_file(), Some(Cow::Borrowed("crates/observed/src/lib.rs")));
        assert_eq!(evaluated.source_line(), Some(42));
        assert_eq!(evaluated.source_crate(), Some(Cow::Borrowed("observed")));
    }
}
