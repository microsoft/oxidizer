use std::sync::{Arc, Mutex};

use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use type_map::concurrent::TypeMap;

use crate::processing::GenericProcessingInstructions;
use crate::{Event, ProcessingInstructions};

pub struct Emitter {
    instructions: Arc<Mutex<TypeMap>>,
    name_instruction_overrides: Arc<Mutex<std::collections::HashMap<String, GenericProcessingInstructions>>>,
    data: EmitterData,
}

impl Emitter {
    pub fn emit<T: Event>(&self, event: T) {
        let name = T::DESCRIPTION.name;
        // TODO: we don't want to do this lookup on every emit - try to do something with generics to avoid it based on benchmarks
        if let Some(instructions) = self.name_instruction_overrides.lock().expect("poisoned lock").get(name) {
            instructions.execute(&event, &self.data);
            return;
        }

        let mut instructions = self.instructions.lock().expect("poisoned lock");
        // TODO: we don't want to do this lookup on every emit - try to do something with generics to avoid it based on benchmarks
        let processing_instructions = instructions
            .entry::<ProcessingInstructions<T>>()
            .or_insert_with(|| T::default_instructions());

        processing_instructions.execute(&event, &self.data);
    }

    pub fn set_processing_instructions<T: Event>(&self, instructions: ProcessingInstructions<T>) {
        let mut map = self.instructions.lock().expect("poisoned lock");
        map.insert(instructions);
    }

    pub fn set_processing_instructions_for_name(&self, name: String, instructions: GenericProcessingInstructions) {
        let mut map = self.name_instruction_overrides.lock().expect("poisoned lock");
        map.insert(name, instructions);
    }
}

pub(crate) struct EmitterData {
    pub(crate) logger_provider: SdkLoggerProvider,
    pub(crate) meter_provider: SdkMeterProvider,
    // TODO: custom type map for user-provided other things?
}
