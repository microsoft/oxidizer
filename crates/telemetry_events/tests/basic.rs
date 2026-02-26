use std::time::Duration;

use data_privacy::simple_redactor::SimpleRedactor;
use data_privacy::{RedactionEngine, classified, taxonomy};
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_stdout::{LogExporter, MetricExporter};
use telemetry_events::{Emitter, EmitterPipeline, Event, InstrumentKind};

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
    log(
        name = "outgoing_request",
        message = "Outgoing request of type {request_type} from org {org_id} with duration {duration} and size {size}, request id {request_id} and operation {operation}"
    ),
    log(
        name = "outgoing_request_summary",
        message = "Outgoing request with duration {duration} and size {size}"
    )
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

#[test]
fn test_event_emission() {
    // Set up OpenTelemetry providers with stdout exporters.
    let logger_provider = SdkLoggerProvider::builder().with_simple_exporter(LogExporter::default()).build();
    let meter_provider = SdkMeterProvider::builder()
        .with_periodic_exporter(MetricExporter::default())
        .build();

    // Build a redaction engine with a passthrough redactor for public data
    // and asterisk replacement for everything else.
    let redaction_engine = RedactionEngine::builder()
        .add_class_redactor(
            DataClass::Public,
            SimpleRedactor::with_mode(data_privacy::simple_redactor::SimpleRedactorMode::Passthrough),
        )
        .build();

    // Wire up the emitter with a single pipeline.
    let pipeline = EmitterPipeline::new(logger_provider.clone(), meter_provider.clone(), redaction_engine);
    let mut emitter = Emitter::new();
    emitter.add_pipeline::<OutgoingRequest>(pipeline);

    // Emit an event — logs and metrics are written to stdout.
    emitter.emit(OutgoingRequest {
        org_id: OrganizationId("org-42".into()),
        request_type: RequestType("graphql".into()),
        request_id: RequestId("req-123".into()),
        operation: OperationType("query".into()),
        duration: Duration::from_millis(150),
        size: 2048,
    });

    logger_provider.force_flush();
    meter_provider.force_flush();
}
