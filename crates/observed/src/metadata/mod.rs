// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Event's metadata.

mod event;
mod field;
mod log;
mod metric;
mod source;

pub use event::EventDescription;
pub use field::{FieldDescriptor, LogFieldEntry, MetricFieldEntry};
pub use log::LogDescription;
pub use metric::{InstrumentKind, MetricDescription};
pub use source::SourceLocation;
