// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;

use tick::Clock;

#[cfg(any(feature = "metrics", test))]
use crate::circuit::telemetry::*;
use crate::circuit::{CircuitEngine, CircuitState, EnterCircuitResult, ExecutionMode, ExecutionResult, ExitCircuitResult};

use crate::utils::TelemetryHelper;
#[cfg(any(feature = "metrics", test))]
use crate::utils::{EVENT_NAME, PIPELINE_NAME, STRATEGY_NAME};

/// Wrapper around a circuit engine to add telemetry capabilities.
#[derive(Debug)]
pub(crate) struct EngineTelemetry<T> {
    inner: T,
    pub(super) telemetry: TelemetryHelper,
    pub(super) partition_key: Cow<'static, str>,
    pub(super) clock: Clock,
}

impl<T> EngineTelemetry<T> {
    pub fn new(inner: T, telemetry: TelemetryHelper, partition_key: Cow<'static, str>, clock: Clock) -> Self {
        Self {
            inner,
            telemetry,
            partition_key,
            clock,
        }
    }
}

impl<T: CircuitEngine> CircuitEngine for EngineTelemetry<T> {
    fn enter(&self) -> EnterCircuitResult {
        let enter_result = self.inner.enter();

        if matches!(enter_result, EnterCircuitResult::Rejected) {
            #[cfg(any(feature = "metrics", test))]
            if let Some(reporter) = &self.telemetry.event_reporter {
                reporter.add(
                    1,
                    &[
                        opentelemetry::KeyValue::new(PIPELINE_NAME, self.telemetry.pipeline_name.clone()),
                        opentelemetry::KeyValue::new(STRATEGY_NAME, self.telemetry.strategy_name.clone()),
                        opentelemetry::KeyValue::new(EVENT_NAME, CIRCUIT_REJECTED_EVENT_NAME),
                        opentelemetry::KeyValue::new(CIRCUIT_STATE, CircuitState::Open.as_str()),
                        opentelemetry::KeyValue::new(CIRCUIT_PARTITION, self.partition_key.clone()),
                    ],
                );
            }

            if self.telemetry.logs_enabled {
                #[cfg(any(feature = "logs", test))]
                tracing::event!(
                    name: "seatbelt.circuit_breaker.rejected",
                    tracing::Level::WARN,
                    pipeline.name = %self.telemetry.pipeline_name,
                    strategy.name = %self.telemetry.strategy_name,
                    circuit_breaker.state = CircuitState::Open.as_str(),
                    circuit_breaker.partition = %self.partition_key,
                );
            }
        }

        enter_result
    }

    fn exit(&self, result: ExecutionResult, mode: ExecutionMode) -> ExitCircuitResult {
        if mode == ExecutionMode::Probe {
            #[cfg(any(feature = "metrics", test))]
            if self.telemetry.metrics_enabled() {
                self.telemetry.report_metrics(&[
                    opentelemetry::KeyValue::new(PIPELINE_NAME, self.telemetry.pipeline_name.clone()),
                    opentelemetry::KeyValue::new(STRATEGY_NAME, self.telemetry.strategy_name.clone()),
                    opentelemetry::KeyValue::new(EVENT_NAME, CIRCUIT_PROBE_EVENT_NAME),
                    opentelemetry::KeyValue::new(CIRCUIT_STATE, CircuitState::HalfOpen.as_str()),
                    opentelemetry::KeyValue::new(CIRCUIT_PARTITION, self.partition_key.clone()),
                    opentelemetry::KeyValue::new(CIRCUIT_PROBE_RESULT, result.as_str()),
                ]);
            }

            #[cfg(any(feature = "logs", test))]
            if self.telemetry.logs_enabled {
                tracing::event!(
                    name: "seatbelt.circuit_breaker.probe",
                    tracing::Level::INFO,
                    pipeline.name = %self.telemetry.pipeline_name,
                    strategy.name = %self.telemetry.strategy_name,
                    circuit_breaker.state = CircuitState::HalfOpen.as_str(),
                    circuit_breaker.partition = %self.partition_key,
                    circuit_breaker.probe.result = result.as_str(),
                );
            }
        }

        let exit_result = self.inner.exit(result, mode);

        // Emit telemetry events for circuit state changes
        match exit_result {
            ExitCircuitResult::Opened(health) => {
                #[cfg(any(feature = "metrics", test))]
                if let Some(reporter) = &self.telemetry.event_reporter {
                    reporter.add(
                        1,
                        &[
                            opentelemetry::KeyValue::new(PIPELINE_NAME, self.telemetry.pipeline_name.clone()),
                            opentelemetry::KeyValue::new(STRATEGY_NAME, self.telemetry.strategy_name.clone()),
                            opentelemetry::KeyValue::new(EVENT_NAME, CIRCUIT_OPENED_EVENT_NAME),
                            opentelemetry::KeyValue::new(CIRCUIT_STATE, CircuitState::Open.as_str()),
                            opentelemetry::KeyValue::new(CIRCUIT_PARTITION, self.partition_key.clone()),
                        ],
                    );
                }

                if self.telemetry.logs_enabled {
                    #[cfg(any(feature = "logs", test))]
                    tracing::event!(
                        name: "seatbelt.circuit_breaker.opened",
                        tracing::Level::WARN,
                        pipeline.name = %self.telemetry.pipeline_name,
                        strategy.name = %self.telemetry.strategy_name,
                        circuit_breaker.state = CircuitState::Open.as_str(),
                        circuit_breaker.partition = %self.partition_key,
                        circuit_breaker.health.failure_rate = health.failure_rate(),
                        circuit_breaker.health.throughput = health.throughput(),
                    );
                }

                _ = health;
            }
            ExitCircuitResult::Closed(ref stats) => {
                #[cfg(any(feature = "metrics", test))]
                if let Some(reporter) = &self.telemetry.event_reporter {
                    reporter.add(
                        1,
                        &[
                            opentelemetry::KeyValue::new(PIPELINE_NAME, self.telemetry.pipeline_name.clone()),
                            opentelemetry::KeyValue::new(STRATEGY_NAME, self.telemetry.strategy_name.clone()),
                            opentelemetry::KeyValue::new(EVENT_NAME, CIRCUIT_CLOSED_EVENT_NAME),
                            opentelemetry::KeyValue::new(CIRCUIT_STATE, CircuitState::Closed.as_str()),
                            opentelemetry::KeyValue::new(CIRCUIT_PARTITION, self.partition_key.clone()),
                        ],
                    );
                }

                if self.telemetry.logs_enabled {
                    #[cfg(any(feature = "logs", test))]
                    tracing::event!(
                        name: "seatbelt.circuit_breaker.closed",
                        tracing::Level::INFO,
                        pipeline.name = %self.telemetry.pipeline_name,
                        strategy.name = %self.telemetry.strategy_name,
                        circuit_breaker.state = CircuitState::Closed.as_str(),
                        circuit_breaker.open.duration = stats.opened_duration(self.clock.instant()).as_secs(),
                        circuit_breaker.partition = %self.partition_key,
                        circuit_breaker.probes.total = stats.probes_total,
                        circuit_breaker.probes.successfull = stats.probes_successes,
                        circuit_breaker.probes.failed = stats.probes_failures,
                        circuit_breaker.probes.lost = stats.probes_lost,
                        circuit_breaker.rejections = stats.rejected,
                        circuit_breaker.re_opened = stats.re_opened,
                    );
                }

                _ = stats;
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

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
#[cfg(not(miri))]
mod tests {
    use std::time::Instant;

    use opentelemetry::KeyValue;

    use super::*;
    use crate::circuit::{EngineFake, HealthInfo, Stats};
    use crate::metrics::{create_meter, create_resilience_event_counter};
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
        let telemetry = TelemetryHelper {
            pipeline_name: "test_pipeline".into(),
            strategy_name: "test_strategy".into(),
            event_reporter: Some(create_resilience_event_counter(&create_meter(tester.meter_provider()))),
            logs_enabled: true,
        };
        let telemetry_engine = EngineTelemetry::new(engine, telemetry, "test_partition".into(), Clock::new_frozen());
        (tester, telemetry_engine)
    }
}
