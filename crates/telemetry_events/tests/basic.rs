use std::time::Duration;

use data_privacy::{RedactionEngine, classified, taxonomy};
use telemetry_events::{Event, EventDescription, FieldDescription, TelemetrySafeValue};

#[taxonomy(ExampleTaxonomy)]
enum DataClass {
    Private,
    Public,
}

#[classified(DataClass::Private)]
struct OrganizationId(String);

struct OutgoingRequest {
    org_id: OrganizationId,
    duration: Duration,
    size: i64,
}

impl Event for OutgoingRequest {
    const DESCRIPTION: telemetry_events::EventDescription = EventDescription {
        name: "outgoing_request",
        id: 0,
        fields: &[
            FieldDescription { name: "org_id", index: 0 },
            FieldDescription {
                name: "duration",
                index: 1,
            },
            FieldDescription { name: "size", index: 2 },
        ],
    };

    fn default_instructions() -> telemetry_events::ProcessingInstructions<Self> {
        todo!()
    }

    fn value(&self, field: &FieldDescription, redaction_engine: &RedactionEngine) -> TelemetrySafeValue {
        match field.index {
            0 => TelemetrySafeValue::from_redacted(&self.org_id, redaction_engine),
            1 => self.duration.as_secs_f64().into(),
            2 => self.size.into(),
            _ => panic!("unknown field index"),
        }
    }
}
