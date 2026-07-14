// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Test harness for the `observed` crate family.
//!
//! Provides a [`MockProcessor`] that captures emitted events for assertion,
//! and helper functions for setting up test emitters.
//!
//! # Usage
//!
//! ```
//! use observed::{Event, Severity, Sink, SinkId, emit};
//! use observed_testing::{ExpectedEvent, test_emitter};
//!
//! static ID: SinkId = SinkId::new("test");
//! let (sink, processor) = test_emitter(ID);
//!
//! # #[derive(Event)]
//! # #[event(name = "my.event")]
//! # #[log(severity = info)]
//! # struct MyEvent { #[unredacted] count: i64 }
//! emit!(sink, MyEvent { count: 42 });
//!
//! let event = processor.single_event();
//! let expected = ExpectedEvent::new("my.event", Severity::Info)
//!     .dimension("count", 42i64)
//!     .log();
//! assert_eq!(event, expected);
//! ```

#![allow(clippy::missing_panics_doc, reason = "Test code doesn't need exhaustive panic docs")]

mod mock_processor;

pub mod events;
pub mod taxonomy;
pub mod types;

use std::sync::Arc;

pub use mock_processor::{
    CapturedEvent, CapturedFieldMetric, ExpectedEnrichmentEntry, ExpectedEvent, ExpectedEventDescription, MockProcessor,
};
use observed::{Sink, SinkId};
pub use taxonomy::MicrosoftEnterpriseDataTaxonomy;
use tick::SimpleClock;

/// Creates a passthrough [`data_privacy::RedactionEngine`] that does not
/// redact any values. Useful for testing `Event::value()` extraction without
/// privacy processing.
#[must_use]
pub fn passthrough_redaction_engine() -> data_privacy::RedactionEngine {
    data_privacy::RedactionEngine::builder()
        .set_fallback_redactor(data_privacy::simple_redactor::SimpleRedactor::with_mode(
            data_privacy::simple_redactor::SimpleRedactorMode::Passthrough,
        ))
        .build()
}

/// Creates a test [`Sink`] backed by a [`MockProcessor`].
///
/// Returns both the sink and a handle to the mock processor for assertions.
/// The processor uses a passthrough redaction engine so classified string values
/// appear unredacted in captured events.
#[must_use]
pub fn test_emitter(id: SinkId) -> (Sink, MockProcessor) {
    let processor = MockProcessor::new();
    let sink = Sink::new(id, vec![Arc::new(processor.clone())], SimpleClock::new_frozen());
    (sink, processor)
}

/// A shared [`SinkId`] used by integration tests that do not need a distinct id.
pub static TEST_ID: SinkId = SinkId::new("test");
