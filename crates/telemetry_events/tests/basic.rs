use std::time::Duration;

use data_privacy::{RedactionEngine, classified, taxonomy};
use telemetry_events::{
    Event, EventDescription, FieldDescription, GenericProcessingInstructions, InstrumentKind, LogProcessingInstructions,
    MetricProcessingInstructions, ProcessingInstructions, TelemetrySafeValue,
};

#[taxonomy(ExampleTaxonomy)]
enum DataClass {
    Private,
    Public,
}

#[classified(DataClass::Private)]
struct OrganizationId(String);

#[classified(DataClass::Public)]
struct RequestId(String);

#[classified(DataClass::Public)]
struct RequestType(String);

#[classified(DataClass::Public)]
struct OperationType(String);

#[derive(Event)]
#[telemetry_events(
    id = 0,
    name = "outgoing_request",
    log(name = "outgoing_request"),
    log(name = "outgoing_request_summary")
)]
struct OutgoingRequest {
    #[telemetry_events(include_in_logs)]
    org_id: OrganizationId,

    #[telemetry_events(include_in_logs, include_in_metrics)]
    request_type: RequestType,

    #[telemetry_events(include_in_log("outgoing_request"))]
    request_id: RequestId,

    #[telemetry_events(include_in_logs, include_in_metric("outgoing_request_size"))]
    operation: OperationType,

    #[telemetry_events(include_in_logs, metric(name = "outgoing_request_duration", kind = InstrumentKind::Histogram))]
    duration: Duration,

    #[telemetry_events(include_in_logs, metric(name = "outgoing_request_size", kind = InstrumentKind::Histogram))]
    size: i64,
}

impl Event for OutgoingRequest {
    const DESCRIPTION: telemetry_events::EventDescription = EventDescription {
        name: "outgoing_request",
        id: 0,
        fields: &[
            FieldDescription { name: "org_id", index: 0 },
            FieldDescription {
                name: "request_type",
                index: 1,
            },
            FieldDescription {
                name: "request_id",
                index: 2,
            },
            FieldDescription {
                name: "operation",
                index: 3,
            },
            FieldDescription {
                name: "duration",
                index: 4,
            },
            FieldDescription { name: "size", index: 5 },
        ],
    };

    fn default_instructions() -> telemetry_events::ProcessingInstructions<Self> {
        ProcessingInstructions {
            generic_instructions: GenericProcessingInstructions {
                log_instructions: vec![
                    LogProcessingInstructions {
                        logger_name: "outgoing_request",
                        included_fields: vec![
                            Self::DESCRIPTION.fields[0],
                            Self::DESCRIPTION.fields[1],
                            Self::DESCRIPTION.fields[2],
                            Self::DESCRIPTION.fields[3],
                            Self::DESCRIPTION.fields[4],
                            Self::DESCRIPTION.fields[5],
                        ],
                        message_template: "Outgoing request of type {request_type} from org {org_id} with duration {duration} and size {size}, request id {request_id} and operation {operation}",
                    },
                    LogProcessingInstructions {
                        logger_name: "outgoing_request_summary",
                        included_fields: vec![
                            Self::DESCRIPTION.fields[0],
                            Self::DESCRIPTION.fields[1],
                            Self::DESCRIPTION.fields[3], // 2 is skipped because there is no include_in_logs, and the include_in_log doesn't match name
                            Self::DESCRIPTION.fields[4],
                            Self::DESCRIPTION.fields[5],
                        ],
                        message_template: "Outgoing request with duration {duration} and size {size}",
                    },
                ],
                metric_instructiosns: vec![
                    MetricProcessingInstructions {
                        meter_name: "outgoing_request",
                        instrument_name: "outgoing_request_duration",
                        included_dimensions: vec![Self::DESCRIPTION.fields[1]],
                        metric_field: Self::DESCRIPTION.fields[4],
                        instrument_kind: InstrumentKind::Histogram,
                    },
                    MetricProcessingInstructions {
                        meter_name: "outgoing_request",
                        instrument_name: "outgoing_request_size",
                        included_dimensions: vec![Self::DESCRIPTION.fields[1], Self::DESCRIPTION.fields[3]],
                        metric_field: Self::DESCRIPTION.fields[5],
                        instrument_kind: InstrumentKind::Histogram,
                    },
                ],
            },
            additional_processing: None,
        }
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
