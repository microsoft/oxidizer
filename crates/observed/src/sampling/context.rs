// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Inputs an event sampler decides from.

use std::any::type_name;
use std::cell::Cell;
use std::fmt::{self, Debug, Formatter};

use performables::arc::Arc;

use crate::metadata::EventDescription;
use crate::processing::EventProcessor;

/// Read-only inputs for one [`EventSampler`](crate::EventSampler) decision.
pub struct EventSamplingContext<'a> {
    description: &'a EventDescription,
    processors: ProcessorInterest<'a>,
}

impl<'a> EventSamplingContext<'a> {
    pub(crate) const fn new(description: &'a EventDescription, processors: &'a [Arc<dyn EventProcessor>]) -> Self {
        Self {
            description,
            processors: ProcessorInterest {
                processors,
                next_processor_index: Cell::new(0),
                bitmask: Cell::new(0),
            },
        }
    }

    pub(crate) fn ask_all_processors(self) -> u32 {
        self.processors.ask_all(self.description)
    }

    /// Returns the event's type-level description.
    #[must_use]
    pub const fn description(&self) -> &EventDescription {
        self.description
    }

    /// Returns whether any of the Sink's processors wants the event.
    ///
    /// The Sink reuses these answers when it delivers the event, so each
    /// processor is asked at most once per event, however often this is called.
    #[must_use]
    pub fn processors_interested(&self) -> bool {
        self.processors.ask_until_one_interested(self.description)
    }
}

struct ProcessorInterest<'a> {
    processors: &'a [Arc<dyn EventProcessor>],
    next_processor_index: Cell<usize>,
    bitmask: Cell<u32>,
}

impl ProcessorInterest<'_> {
    fn ask_all(&self, description: &EventDescription) -> u32 {
        while self.next_processor_index.get() < self.processors.len() {
            self.ask_next_processor(description);
        }
        self.bitmask.get()
    }

    fn ask_until_one_interested(&self, description: &EventDescription) -> bool {
        while self.bitmask.get() == 0 && self.next_processor_index.get() < self.processors.len() {
            self.ask_next_processor(description);
        }
        self.bitmask.get() != 0
    }

    #[cfg_attr(test, mutants::skip)] // Each bit is set once, so `^` equals `|`, and not advancing the index hangs the asking loops.
    fn ask_next_processor(&self, description: &EventDescription) {
        let index = self.next_processor_index.get();
        self.next_processor_index.set(index + 1);
        if self.processors[index].is_interested(description) {
            self.bitmask.set(self.bitmask.get() | 1 << index);
        }
    }
}

impl Debug for EventSamplingContext<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("description", self.description)
            .finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::FlushError;
    use crate::processing::EventView;

    /// A processor that wants every event or none, counting how often it is asked.
    struct CountingProcessor {
        wants: bool,
        asked: AtomicUsize,
    }

    impl EventProcessor for CountingProcessor {
        fn is_interested(&self, _description: &EventDescription) -> bool {
            self.asked.fetch_add(1, Ordering::Relaxed);
            self.wants
        }

        fn process(&self, _event: &EventView<'_>) {}

        fn flush(&self) -> Result<(), FlushError> {
            Ok(())
        }
    }

    fn counting_processors(wants: &[bool]) -> Vec<StdArc<CountingProcessor>> {
        wants
            .iter()
            .map(|&wants| {
                StdArc::new(CountingProcessor {
                    wants,
                    asked: AtomicUsize::new(0),
                })
            })
            .collect()
    }

    fn as_event_processors(processors: &[StdArc<CountingProcessor>]) -> Vec<Arc<dyn EventProcessor>> {
        processors
            .iter()
            .map(|processor| Arc::from_std(StdArc::clone(processor) as StdArc<dyn EventProcessor>))
            .collect()
    }

    fn asked(processors: &[StdArc<CountingProcessor>]) -> Vec<usize> {
        processors.iter().map(|processor| processor.asked.load(Ordering::Relaxed)).collect()
    }

    fn description() -> EventDescription {
        EventDescription::new("probe.event", None, None, None, false, false)
    }

    #[test]
    fn processors_interested_stops_at_first_interested_processor() {
        let processors = counting_processors(&[false, true, true]);
        let event_processors = as_event_processors(&processors);
        let description = description();
        let context = EventSamplingContext::new(&description, &event_processors);

        assert!(context.processors_interested());
        assert_eq!(asked(&processors), [1, 1, 0]);
    }

    #[test]
    fn processors_interested_asks_each_processor_once() {
        let processors = counting_processors(&[false, false]);
        let event_processors = as_event_processors(&processors);
        let description = description();
        let context = EventSamplingContext::new(&description, &event_processors);

        assert!(!context.processors_interested());
        assert!(!context.processors_interested());
        assert_eq!(asked(&processors), [1, 1]);
    }

    #[test]
    fn ask_all_processors_returns_bitmask_of_interested_processors() {
        let processors = counting_processors(&[false, true, false, true]);
        let event_processors = as_event_processors(&processors);
        let description = description();
        let context = EventSamplingContext::new(&description, &event_processors);

        assert_eq!(context.ask_all_processors(), 0b1010);
        assert_eq!(asked(&processors), [1, 1, 1, 1]);
    }

    #[test]
    fn ask_all_processors_skips_processors_already_asked() {
        let processors = counting_processors(&[true, false, true]);
        let event_processors = as_event_processors(&processors);
        let description = description();
        let context = EventSamplingContext::new(&description, &event_processors);

        assert!(context.processors_interested());
        assert_eq!(context.ask_all_processors(), 0b101);
        assert_eq!(asked(&processors), [1, 1, 1]);
    }

    #[test]
    fn debug_shows_description() {
        let description = description();
        let context = EventSamplingContext::new(&description, &[]);

        let debug = format!("{context:?}");

        assert!(debug.contains("EventSamplingContext"));
        assert!(debug.contains("probe.event"));
    }
}
