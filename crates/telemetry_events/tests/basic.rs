use std::time::Duration;

use data_privacy::{classified, taxonomy};
use telemetry_events::{Event, InstrumentKind};

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
    log(name = "outgoing_request", message = "Outgoing request of type {request_type} from org {org_id} with duration {duration} and size {size}, request id {request_id} and operation {operation}"),
    log(name = "outgoing_request_summary", message = "Outgoing request with duration {duration} and size {size}")
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
