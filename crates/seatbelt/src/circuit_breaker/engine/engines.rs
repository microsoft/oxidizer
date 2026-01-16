// Copyright (c) Microsoft Corporation.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use opentelemetry::StringValue;
use opentelemetry::metrics::Counter;
use tick::Clock;

use crate::circuit_breaker::constants::ERR_POISONED_LOCK;
use crate::circuit_breaker::{Engine, EngineCore, EngineOptions, EngineTelemetry, PartitionKey};

/// Manages circuit breaker engines for different partition keys.
#[derive(Debug)]
pub(crate) struct Engines {
    map: Mutex<HashMap<PartitionKey, Arc<Engine>>>,
    engine_options: EngineOptions,
    clock: Clock,
    strategy_name: StringValue,
    pipeline_name: StringValue,
    resilience_event_counter: Counter<u64>,
}

impl Engines {
    pub fn new(
        engine_options: EngineOptions,
        clock: Clock,
        strategy_name: StringValue,
        pipeline_name: StringValue,
        resilience_event_counter: Counter<u64>,
    ) -> Self {
        Self {
            map: Mutex::new(HashMap::new()),
            engine_options,
            clock,
            strategy_name,
            pipeline_name,
            resilience_event_counter,
        }
    }

    pub fn get_engine(&self, key: &PartitionKey) -> Arc<Engine> {
        let mut map = self.map.lock().expect(ERR_POISONED_LOCK);

        if let Some(engine) = map.get(key) {
            return Arc::clone(engine);
        }

        let engine = Arc::new(self.create_engine(key));
        map.insert(key.clone(), Arc::clone(&engine));
        engine
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        let map = self.map.lock().expect(ERR_POISONED_LOCK);
        map.len()
    }

    fn create_engine(&self, key: &PartitionKey) -> Engine {
        EngineTelemetry::new(
            EngineCore::new(self.engine_options.clone(), self.clock.clone()),
            self.strategy_name.clone(),
            self.pipeline_name.clone(),
            key.to_string().into(),
            self.resilience_event_counter.clone(),
            self.clock.clone(),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::circuit_breaker::HealthMetricsBuilder;
    use crate::circuit_breaker::engine::probing::ProbesOptions;
    use crate::telemetry::metrics::create_resilience_event_counter;

    #[test]
    fn get_engine_ok() {
        let engines = Engines::new(
            EngineOptions {
                break_duration: Duration::from_secs(60),
                health_metrics_builder: HealthMetricsBuilder::new(
                    Duration::from_millis(100),
                    0.5,
                    5,
                ),
                probes: ProbesOptions::quick(Duration::from_secs(1)),
            },
            Clock::new_frozen(),
            StringValue::from("strategy"),
            StringValue::from("pipeline"),
            create_resilience_event_counter(&opentelemetry::global::meter("test")),
        );

        assert!(Arc::ptr_eq(
            &engines.get_engine(&PartitionKey::from("test")),
            &engines.get_engine(&PartitionKey::from("test"))
        ));
        assert_eq!(engines.len(), 1);

        _ = engines.get_engine(&PartitionKey::from("test2"));
        assert_eq!(engines.len(), 2);
    }
}
