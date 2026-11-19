// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The event printer that the examples share.

use std::ops::ControlFlow;

use data_privacy::RedactionEngine;
use observed::FlushError;
use observed::metadata::{EventDescription, FieldDescriptor};
use observed::processing::{EventProcessor, EventView, FieldValueFn};

/// Prints the name, the severity, the message and the log fields of each event. Then it prints
/// each metric value with the name, the unit and the dimensions of its metric.
#[derive(Default)]
pub(crate) struct PrintProcessor(RedactionEngine);

impl EventProcessor for PrintProcessor {
    fn is_interested(&self, _: &EventDescription) -> bool {
        true
    }

    fn process(&self, event: &EventView<'_>) {
        let severity = event.severity().map_or("-", |severity| severity.as_str());
        println!("[{severity}] {}", event.name());
        if let Some(message) = event.body() {
            println!("  message: {message}");
        }
        let mut dimensions = Vec::new();
        let mut measurements = Vec::new();
        let _ = event.visit_fields(&mut |descriptor: &FieldDescriptor, value: &FieldValueFn<'_>| {
            let value = value(&self.0);
            if let Some(log) = descriptor.log() {
                println!("  {} = {value}", log.key());
            }
            if let Some(metric) = descriptor.metric() {
                match metric.instrument_description() {
                    Some(instrument) => measurements.push((instrument.instrument_name(), instrument.unit(), value)),
                    None => dimensions.push(format!("{}={value}", metric.key())),
                }
            }
            ControlFlow::Continue(())
        });
        for (instrument, unit, value) in measurements {
            println!("  metric {instrument} = {value} {unit} [{}]", dimensions.join(", "));
        }
    }

    fn flush(&self) -> Result<(), FlushError> {
        Ok(())
    }
}
