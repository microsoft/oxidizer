use std::any::Any;

use opentelemetry::Value;

use crate::description::FieldDescription;
use crate::{EventDescription, ProcessingInstructions};

pub trait Event: Any + Send + Sync {
    const DESCRIPTION: EventDescription;

    fn default_instructions() -> ProcessingInstructions<Self>;

    /// Note on the return type - there could be two options here:
    /// * OpenTelemetry's Value
    /// * OpentTelmetry's AnyValue
    ///
    /// AnyValue supports recursive structures that we don't really want our events to have, so
    /// we stick to Value.
    fn value(&self, field: &FieldDescription) -> Value;
}
