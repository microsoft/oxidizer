use opentelemetry::logs::{AnyValue, LogRecord, Logger, LoggerProvider};
use opentelemetry::metrics::MeterProvider;
use opentelemetry::{KeyValue, StringValue, Value};
use opentelemetry_sdk::logs::SdkLoggerProvider;

pub enum InstrumentKind {
    UpDownCounter,
    Gauge,
    Histogram,
}

use crate::Event;
use crate::description::FieldDescription;
use crate::emitter::EmitterData;

pub struct ProcessingInstructions<T>
where
    T: ?Sized,
{
    generic_instructions: GenericProcessingInstructions,
    additional_processing: Option<Box<dyn Fn(&T) + Send + Sync>>,
}

impl<T: Event> ProcessingInstructions<T> {
    pub fn execute(&self, event: &T, emitter_data: &EmitterData) {
        if let Some(additional_processing) = &self.additional_processing {
            additional_processing(event);
        }

        self.generic_instructions.execute(event, emitter_data);
    }
}

pub struct GenericProcessingInstructions {
    log_instructions: Option<LogProcessingInstructions>,
    metric_instructiosns: Option<MetricProcessingInstructions>,
}

impl GenericProcessingInstructions {
    pub fn execute<T: Event>(&self, event: &T, emitter_data: &EmitterData) {
        if let Some(log_instructions) = &self.log_instructions {
            log_instructions.execute(event, &emitter_data.logger_provider);
        }

        if let Some(metric_instructions) = &self.metric_instructiosns {
            metric_instructions.execute(event, &emitter_data.meter_provider);
        }
    }
}

pub struct LogProcessingInstructions {
    logger_name: &'static str,
    included_fields: Vec<FieldDescription>,
    message_template: &'static str,
}

impl LogProcessingInstructions {
    pub fn execute<T: Event>(&self, event: &T, logger_provider: &SdkLoggerProvider) {
        // TODO: this should probably be cached - benchmark first
        let logger = logger_provider.logger(self.logger_name);

        let description = T::DESCRIPTION;
        let mut record = logger.create_log_record();
        record.set_body(AnyValue::String(StringValue::from(self.message_template)));
        record.set_event_name(description.name);
        for field in &self.included_fields {
            record.add_attribute(field.name, convert_to_any_value(event.value(&field)));
        }
        // TODO: other fields
        logger.emit(record);
    }
}

pub struct MetricProcessingInstructions {
    meter_name: &'static str,
    instrument_name: &'static str,
    included_dimensions: Vec<FieldDescription>,
    metric_field: FieldDescription,
    instrument_kind: InstrumentKind,
}

impl MetricProcessingInstructions {
    fn execute<T: Event>(&self, event: &T, meter_provider: &opentelemetry_sdk::metrics::SdkMeterProvider) {
        let value = match event.value(&self.metric_field) {
            Value::I64(i) => i as f64,
            Value::F64(f) => f,
            _ => {
                return; // TODO: what to do here?
            }
        };

        // TODO: avoid allocations - we can probably pre-create a Vec of KeyValue with the right keys and just update the values here
        let attributes = self
            .included_dimensions
            .iter()
            .map(|field| KeyValue::new(field.name, event.value(field)))
            .collect::<Vec<_>>();

        // TODO: cache meters and instruments - benchmark first
        let meter = meter_provider.meter(self.meter_name);
        match self.instrument_kind {
            InstrumentKind::UpDownCounter => {
                let counter = meter.f64_up_down_counter(self.instrument_name).build();
                counter.add(value, &attributes);
            }
            InstrumentKind::Histogram => {
                let histogram = meter.f64_histogram(self.instrument_name).build();
                histogram.record(value, &attributes);
            }
            InstrumentKind::Gauge => {
                let gauge = meter.f64_gauge(self.instrument_name).build();
                gauge.record(value, &attributes);
            }
        }
    }
}

fn convert_to_any_value(value: Value) -> AnyValue {
    match value {
        Value::String(s) => AnyValue::String(StringValue::from(s)),
        Value::Bool(b) => AnyValue::Boolean(b),
        Value::I64(i) => AnyValue::Int(i),
        Value::F64(f) => AnyValue::Double(f),
        Value::Array(arr) => AnyValue::ListAny(Box::new(match arr {
            opentelemetry::Array::Bool(items) => items.into_iter().map(AnyValue::Boolean).collect(),
            opentelemetry::Array::I64(items) => items.into_iter().map(AnyValue::Int).collect(),
            opentelemetry::Array::F64(items) => items.into_iter().map(AnyValue::Double).collect(),
            opentelemetry::Array::String(items) => items.into_iter().map(AnyValue::String).collect(),
            _ => vec![],
        })),
        x => AnyValue::String(StringValue::from(x.to_string())),
    }
}
