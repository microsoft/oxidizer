// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use tick::Clock;

use crate::breaker::constants::ERR_POISONED_LOCK;
use crate::breaker::{BreakerId, Engine, EngineCore, EngineOptions, EngineTelemetry};
use crate::utils::TelemetryHelper;

/// Manages circuit breaker engines for different breaker IDs.
#[derive(Debug)]
pub(crate) struct Engines {
    default_engine: Arc<Engine>,
    map: RwLock<BTreeMap<BreakerId, Arc<Engine>>>,
    engine_options: EngineOptions,
    clock: Clock,
    telemetry: TelemetryHelper,
}

impl Engines {
    pub(crate) fn new(engine_options: EngineOptions, clock: Clock, telemetry: TelemetryHelper) -> Self {
        let default_engine = Arc::new(create_engine(&engine_options, &clock, &telemetry, &BreakerId::default()));
        Self {
            default_engine,
            map: RwLock::new(BTreeMap::new()),
            engine_options,
            clock,
            telemetry,
        }
    }

    pub(crate) fn get_engine(&self, key: &BreakerId) -> Arc<Engine> {
        // Fast path: the default breaker (the common single-breaker configuration, used
        // whenever no ID provider is configured) is served by a pre-created engine. This
        // avoids a lock and the map lookup entirely on every call.
        if key.is_default() {
            return Arc::clone(&self.default_engine);
        }

        // Read-lock path for existing engines (common case). The map is a `BTreeMap`, so a
        // lookup is a handful of key comparisons rather than a hash of `key`; partitioned
        // breakers are low-cardinality by design, so this stays cheap and, unlike a hash
        // map, is not exposed to hash-flooding via request-derived IDs.
        {
            let map = self.map.read().expect(ERR_POISONED_LOCK);
            if let Some(engine) = map.get(key) {
                return Arc::clone(engine);
            }
        }

        // Slow path: acquire write lock to insert a new engine.
        let mut map = self.map.write().expect(ERR_POISONED_LOCK);
        let engine = map
            .entry(key.clone())
            .or_insert_with(|| Arc::new(create_engine(&self.engine_options, &self.clock, &self.telemetry, key)));

        Arc::clone(engine)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        let map = self.map.read().expect(ERR_POISONED_LOCK);
        map.len()
    }
}

fn create_engine(engine_options: &EngineOptions, clock: &Clock, telemetry: &TelemetryHelper, key: &BreakerId) -> Engine {
    EngineTelemetry::new(
        EngineCore::new(engine_options.clone(), clock.clone()),
        telemetry.clone(),
        key.clone().into(),
        clock.clone(),
    )
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::breaker::engine::probing::ProbesOptions;
    use crate::breaker::{AbandonedPolicy, HealthMetricsBuilder};
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
                break_duration: Duration::from_mins(1),
                health_metrics_builder: HealthMetricsBuilder::new(Duration::from_millis(100), 0.5, 5, AbandonedPolicy::default()),
                probes: ProbesOptions::quick(Duration::from_secs(1), &AbandonedPolicy::default()),
            },
            Clock::new_frozen(),
            telemetry,
        );

        assert!(Arc::ptr_eq(
            &engines.get_engine(&BreakerId::from("test")),
            &engines.get_engine(&BreakerId::from("test"))
        ));
        assert_eq!(engines.len(), 1);

        _ = engines.get_engine(&BreakerId::from("test2"));
        assert_eq!(engines.len(), 2);
    }
}
