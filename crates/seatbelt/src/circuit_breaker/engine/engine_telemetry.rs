// Copyright (c) Microsoft Corporation.

use opentelemetry::StringValue;
use opentelemetry::metrics::Counter;
use tick::Clock;

use crate::circuit_breaker::telemetry::*;
use crate::circuit_breaker::{
    CircuitEngine, CircuitState, EnterCircuitResult, ExecutionMode, ExecutionResult,
    ExitCircuitResult,
};
use crate::telemetry::{EVENT_NAME, PIPELINE_NAME, STRATEGY_NAME};

/// Wrapper around a circuit engine to add telemetry capabilities.
#[derive(Debug)]
pub(crate) struct EngineTelemetry<T> {
    inner: T,
    pub(super) strategy_name: StringValue,
    pub(super) pipeline_name: StringValue,
    pub(super) resilience_events: Counter<u64>,
    pub(super) partition_key: StringValue,
    pub(super) clock: Clock,
}

impl<T> EngineTelemetry<T> {
    pub fn new(
        inner: T,
        strategy_name: StringValue,
        pipeline_name: StringValue,
        partition_key: StringValue,
        resilience_events: Counter<u64>,
        clock: Clock,
    ) -> Self {
        Self {
            inner,
            strategy_name,
            pipeline_name,
            resilience_events,
            partition_key,
            clock,
        }
    }
}

impl<T: CircuitEngine> CircuitEngine for EngineTelemetry<T> {
    fn enter(&self) -> EnterCircuitResult {
        let enter_result = self.inner.enter();

        if matches!(enter_result, EnterCircuitResult::Rejected) {
            self.resilience_events.add(
                1,
                &[
                    opentelemetry::KeyValue::new(PIPELINE_NAME, self.pipeline_name.clone()),
                    opentelemetry::KeyValue::new(STRATEGY_NAME, self.strategy_name.clone()),
                    opentelemetry::KeyValue::new(EVENT_NAME, CIRCUIT_REJECTED_EVENT_NAME),
                    opentelemetry::KeyValue::new(CIRCUIT_STATE, CircuitState::Open.as_str()),
                    opentelemetry::KeyValue::new(CIRCUIT_PARTITION, self.partition_key.clone()),
                ],
            );

            tracing::event!(
                name: "seatbelt.circuit_breaker.rejected",
                tracing::Level::WARN,
                pipeline.name = self.pipeline_name.as_str(),
                strategy.name = self.strategy_name.as_str(),
                circuit_breaker.state = CircuitState::Open.as_str(),
                circuit_breaker.partition = self.partition_key.as_str(),
            );
        }

        enter_result
    }

    fn exit(&self, result: ExecutionResult, mode: ExecutionMode) -> ExitCircuitResult {
        if mode == ExecutionMode::Probe {
            self.resilience_events.add(
                1,
                &[
                    opentelemetry::KeyValue::new(PIPELINE_NAME, self.pipeline_name.clone()),
                    opentelemetry::KeyValue::new(STRATEGY_NAME, self.strategy_name.clone()),
                    opentelemetry::KeyValue::new(EVENT_NAME, CIRCUIT_PROBE_EVENT_NAME),
                    opentelemetry::KeyValue::new(CIRCUIT_STATE, CircuitState::HalfOpen.as_str()),
                    opentelemetry::KeyValue::new(CIRCUIT_PARTITION, self.partition_key.clone()),
                    opentelemetry::KeyValue::new(CIRCUIT_PROBE_RESULT, result.as_str()),
                ],
            );

            tracing::event!(
                name: "seatbelt.circuit_breaker.probe",
                tracing::Level::INFO,
                pipeline.name = self.pipeline_name.as_str(),
                strategy.name = self.strategy_name.as_str(),
                circuit_breaker.state = CircuitState::HalfOpen.as_str(),
                circuit_breaker.partition = self.partition_key.as_str(),
                circuit_breaker.probe.result = result.as_str(),
            );
        }

        let exit_result = self.inner.exit(result, mode);

        // Emit telemetry events for circuit state changes
        match exit_result {
            ExitCircuitResult::Opened(health) => {
                self.resilience_events.add(
                    1,
                    &[
                        opentelemetry::KeyValue::new(PIPELINE_NAME, self.pipeline_name.clone()),
                        opentelemetry::KeyValue::new(STRATEGY_NAME, self.strategy_name.clone()),
                        opentelemetry::KeyValue::new(EVENT_NAME, CIRCUIT_OPENED_EVENT_NAME),
                        opentelemetry::KeyValue::new(CIRCUIT_STATE, CircuitState::Open.as_str()),
                        opentelemetry::KeyValue::new(CIRCUIT_PARTITION, self.partition_key.clone()),
                    ],
                );

                tracing::event!(
                    name: "seatbelt.circuit_breaker.opened",
                    tracing::Level::WARN,
                    pipeline.name = self.pipeline_name.as_str(),
                    strategy.name = self.strategy_name.as_str(),
                    circuit_breaker.state = CircuitState::Open.as_str(),
                    circuit_breaker.partition = self.partition_key.as_str(),
                    circuit_breaker.health.failure_rate = health.failure_rate(),
                    circuit_breaker.health.throughput = health.throughput(),
                );
            }
            ExitCircuitResult::Closed(ref stats) => {
                self.resilience_events.add(
                    1,
                    &[
                        opentelemetry::KeyValue::new(PIPELINE_NAME, self.pipeline_name.clone()),
                        opentelemetry::KeyValue::new(STRATEGY_NAME, self.strategy_name.clone()),
                        opentelemetry::KeyValue::new(EVENT_NAME, CIRCUIT_CLOSED_EVENT_NAME),
                        opentelemetry::KeyValue::new(CIRCUIT_STATE, CircuitState::Closed.as_str()),
                        opentelemetry::KeyValue::new(CIRCUIT_PARTITION, self.partition_key.clone()),
                    ],
                );

                tracing::event!(
                    name: "seatbelt.circuit_breaker.closed",
                    tracing::Level::INFO,
                    pipeline.name = self.pipeline_name.as_str(),
                    strategy.name = self.strategy_name.as_str(),
                    circuit_breaker.state = CircuitState::Closed.as_str(),
                    circuit_breaker.open.duration = stats.opened_duration(self.clock.instant()).as_secs(),
                    circuit_breaker.partition = self.partition_key.as_str(),
                    circuit_breaker.probes.total = stats.probes_total,
                    circuit_breaker.probes.successfull = stats.probes_successes,
                    circuit_breaker.probes.failed = stats.probes_failures,
                    circuit_breaker.probes.lost = stats.probes_lost,
                    circuit_breaker.rejections = stats.rejected,
                    circuit_breaker.re_opened = stats.re_opened,
                );
            }
            ExitCircuitResult::Reopened | ExitCircuitResult::Unchanged => {
                // We do not report a telemetry event for reopening the circuit
                // as it is redundant because it is always preceded by an "opened"
                // event, or when there is no state change.
            }
        }

        exit_result
    }
}

#[cfg(test)]
#[cfg(not(miri))]
mod tests {
    use std::time::Instant;

    use opentelemetry::KeyValue;

    use super::*;
    use crate::circuit_breaker::{EngineFake, HealthInfo, Stats};
    use crate::telemetry::metrics::{create_meter, create_resilience_event_counter};
    use crate::testing::MetricTester;

    #[test]
    fn enter_rejected_ensure_telemetry() {
        let (tester, telemetry_engine) = create_engine(EngineFake::new(
            EnterCircuitResult::Rejected,
            ExitCircuitResult::Closed(Stats::new(Instant::now())),
        ));

        let _ = telemetry_engine.enter();

        tester.assert_attributes(
            &[
                KeyValue::new(PIPELINE_NAME, "test_pipeline"),
                KeyValue::new(STRATEGY_NAME, "test_strategy"),
                KeyValue::new(EVENT_NAME, CIRCUIT_REJECTED_EVENT_NAME),
                KeyValue::new(CIRCUIT_PARTITION, "test_partition"),
                KeyValue::new(CIRCUIT_STATE, CircuitState::Open.as_str()),
            ],
            Some(5),
        );
    }

    #[test]
    fn exit_probe_ensure_telemetry() {
        let (tester, telemetry_engine) = create_engine(EngineFake::new(
            EnterCircuitResult::Accepted {
                mode: ExecutionMode::Normal,
            },
            ExitCircuitResult::Unchanged,
        ));

        let _ = telemetry_engine.exit(ExecutionResult::Success, ExecutionMode::Probe);

        tester.assert_attributes(
            &[
                KeyValue::new(PIPELINE_NAME, "test_pipeline"),
                KeyValue::new(STRATEGY_NAME, "test_strategy"),
                KeyValue::new(EVENT_NAME, CIRCUIT_PROBE_EVENT_NAME),
                KeyValue::new(CIRCUIT_PARTITION, "test_partition"),
                KeyValue::new(CIRCUIT_STATE, CircuitState::HalfOpen.as_str()),
                KeyValue::new(CIRCUIT_PROBE_RESULT, ExecutionResult::Success.as_str()),
            ],
            Some(6),
        );
    }

    #[test]
    fn circuit_closed_ensure_telemetry() {
        let (tester, telemetry_engine) = create_engine(EngineFake::new(
            EnterCircuitResult::Accepted {
                mode: ExecutionMode::Normal,
            },
            ExitCircuitResult::Closed(Stats::new(Instant::now())),
        ));

        let _ = telemetry_engine.exit(ExecutionResult::Success, ExecutionMode::Normal);

        tester.assert_attributes(
            &[
                KeyValue::new(PIPELINE_NAME, "test_pipeline"),
                KeyValue::new(STRATEGY_NAME, "test_strategy"),
                KeyValue::new(EVENT_NAME, CIRCUIT_CLOSED_EVENT_NAME),
                KeyValue::new(CIRCUIT_PARTITION, "test_partition"),
                KeyValue::new(CIRCUIT_STATE, CircuitState::Closed.as_str()),
            ],
            Some(5),
        );
    }

    #[test]
    fn circuit_opened_ensure_telemetry() {
        let (tester, telemetry_engine) = create_engine(EngineFake::new(
            EnterCircuitResult::Accepted {
                mode: ExecutionMode::Normal,
            },
            ExitCircuitResult::Opened(HealthInfo::new(1, 0, 0.75, 100)),
        ));

        let _ = telemetry_engine.exit(ExecutionResult::Failure, ExecutionMode::Normal);
        tester.assert_attributes(
            &[
                KeyValue::new(PIPELINE_NAME, "test_pipeline"),
                KeyValue::new(STRATEGY_NAME, "test_strategy"),
                KeyValue::new(EVENT_NAME, CIRCUIT_OPENED_EVENT_NAME),
                KeyValue::new(CIRCUIT_PARTITION, "test_partition"),
                KeyValue::new(CIRCUIT_STATE, CircuitState::Open.as_str()),
            ],
            Some(5),
        );
    }

    fn create_engine(engine: EngineFake) -> (MetricTester, EngineTelemetry<EngineFake>) {
        let tester = MetricTester::new();
        let telemetry_engine = EngineTelemetry::new(
            engine,
            "test_strategy".into(),
            "test_pipeline".into(),
            "test_partition".into(),
            create_resilience_event_counter(&create_meter(tester.meter_provider())),
            Clock::new_frozen(),
        );
        (tester, telemetry_engine)
    }
}
