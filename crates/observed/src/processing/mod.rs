// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Processing side: the contract and the view handed to event processors.
//!
//! These types exist solely to *consume* emitted events. An [`EventView`] is
//! built by the emission infrastructure and passed to
//! [`EventProcessor::process`]; concrete processors live in separate
//! destination crates.

mod event_state;
mod event_view;
mod processor;

use std::ops::ControlFlow;

pub(crate) use event_state::IntermediateEvent;
pub use event_view::EventView;
pub use processor::EventProcessor;

use crate::Value;
use crate::metadata::FieldDescriptor;

/// Getter closure passed to [`FieldVisitorFn`].
///
/// Takes a [`data_privacy::RedactionEngine`] reference and returns the
/// redacted [`Value`]. Only call it when the processor actually needs the value.
pub type FieldValueFn<'a> = dyn Fn(&data_privacy::RedactionEngine) -> Value + 'a;

/// Visitor callback for lazily iterating over event fields or enrichment entries.
///
/// Each invocation receives a [`FieldDescriptor`] and a [`FieldValueFn`] getter.
/// The getter takes a [`data_privacy::RedactionEngine`] reference and returns the
/// redacted [`Value`] - it is only called if the processor needs the value.
///
/// Return [`ControlFlow::Continue`] to keep iterating, or
/// [`ControlFlow::Break`] to stop early.
pub type FieldVisitorFn<'a> = dyn FnMut(&FieldDescriptor, &FieldValueFn<'_>) -> ControlFlow<()> + 'a;
