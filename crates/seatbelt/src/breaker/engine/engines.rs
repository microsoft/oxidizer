// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tick::Clock;

use crate::breaker::constants::ERR_POISONED_LOCK;
use crate::breaker::{Engine, EngineCore, EngineOptions, EngineTelemetry, PartitionKey};
use crate::utils::TelemetryHelper;

/// Manages circuit breaker engines for different partition keys.
#[derive(Debug)]
pub(crate) struct Engines {
    map: Mutex<HashMap<PartitionKey, Arc<Engine>>>,
    engine_options: EngineOptions,
    clock: Clock,
    telemetry: TelemetryHelper,
}

impl Engines {
    pub fn new(engine_options: EngineOptions, clock: Clock, telemetry: TelemetryHelper) -> Self {
        Self {
            map: Mutex::new(HashMap::new()),
            engine_options,
            clock,
            telemetry,
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
            self.telemetry.clone(),
            key.clone().into(),
            self.clock.clone(),
        )
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::breaker::HealthMetricsBuilder;
    use crate::breaker::engine::probing::ProbesOptions;
    use crate::metrics::create_resilience_event_counter;

    #[test]
    fn get_engine_ok() {
        let telemetry = TelemetryHelper {
            pipeline_name: "pipeline".into(),
            strategy_name: "strategy".into(),
            event_reporter: Some(create_resilience_event_counter(&opentelemetry::global::meter("test"))),
            logs_enabled: false,
        };
        let engines = Engines::new(
            EngineOptions {
                break_duration: Duration::from_secs(60),
                health_metrics_builder: HealthMetricsBuilder::new(Duration::from_millis(100), 0.5, 5),
                probes: ProbesOptions::quick(Duration::from_secs(1)),
            },
            Clock::new_frozen(),
            telemetry,
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
