// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Lazy typed event construction and its type-erased view.

use std::borrow::Cow;
use std::ops::ControlFlow;

use crate::Event;
use crate::interop::DynEvent;
use crate::metadata::{EventDescription, LogDescription, SourceLocation};
use crate::processing::FieldVisitorFn;

/// Holds a typed event builder until processor interest permits construction.
pub(crate) struct IntermediateEvent<F> {
    build: F,
    source_location: SourceLocation,
}

impl<T: Event, F: FnOnce() -> T> IntermediateEvent<F> {
    pub(crate) fn typed(build: F, source_location: SourceLocation) -> Self {
        Self { build, source_location }
    }

    /// Evaluates the event and returns an [`EvaluatedEvent`] that
    /// implements [`DynEvent`] and can be passed directly to processors.
    pub(crate) fn evaluate(self) -> EvaluatedEvent<T> {
        EvaluatedEvent {
            event: (self.build)(),
            source_location: self.source_location,
        }
    }
}

/// Adapts a constructed typed event and its source location to [`DynEvent`].
pub(crate) struct EvaluatedEvent<T: Event> {
    event: T,
    source_location: SourceLocation,
}

impl<T: Event> DynEvent for EvaluatedEvent<T> {
    fn name(&self) -> &'static str {
        T::DESCRIPTION.name()
    }

    fn body(&self) -> Option<Cow<'static, str>> {
        T::DESCRIPTION.log().and_then(LogDescription::body).map(Cow::Borrowed)
    }

    fn source_file(&self) -> Option<Cow<'static, str>> {
        Some(Cow::Borrowed(self.source_location.file()))
    }

    fn source_line(&self) -> Option<u32> {
        Some(self.source_location.line())
    }

    fn source_crate(&self) -> Option<Cow<'static, str>> {
        Some(Cow::Borrowed(self.source_location.crate_name()))
    }

    fn visit_fields(&self, visitor: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
        self.event.visit_fields(visitor)
    }

    fn description(&self) -> EventDescription {
        T::DESCRIPTION
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
        let evaluated = EvaluatedEvent {
            event: TestEvent,
            source_location: SourceLocation::new("observed", "crates/observed/src/lib.rs", 42),
        };

        assert_eq!(evaluated.name(), "test.event");
        assert_eq!(evaluated.source_file(), Some(Cow::Borrowed("crates/observed/src/lib.rs")));
        assert_eq!(evaluated.source_line(), Some(42));
        assert_eq!(evaluated.source_crate(), Some(Cow::Borrowed("observed")));
    }
}
