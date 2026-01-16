// Copyright (c) Microsoft Corporation.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tick::Clock;

use super::{EngineOptions, EnterCircuitResult, ExitCircuitResult};
use crate::circuit_breaker::constants::ERR_POISONED_LOCK;
use crate::circuit_breaker::engine::probing::{AllowProbeResult, Probes, ProbingResult};
use crate::circuit_breaker::{CircuitEngine, ExecutionMode, ExecutionResult, HealthMetrics, HealthStatus};

/// Engine that manages the state of the circuit breaker.
#[derive(Debug)]
pub(crate) struct EngineCore {
    state: Mutex<State>,
    options: EngineOptions,
    clock: Clock,
}

impl EngineCore {
    pub fn new(options: EngineOptions, clock: Clock) -> Self {
        Self {
            state: Mutex::new(State::Closed {
                health: options.health_metrics_builder.build(),
            }),
            options,
            clock,
        }
    }
}

impl CircuitEngine for EngineCore {
    fn enter(&self) -> EnterCircuitResult {
        let now = self.clock.instant();

        // NOTE: Remember to execute all expensive operations (like time checks) outside the lock.
        self.state.lock().expect(ERR_POISONED_LOCK).enter(now, &self.options)
    }

    fn exit(&self, result: ExecutionResult, _mode: ExecutionMode) -> ExitCircuitResult {
        let now = self.clock.instant();

        // NOTE: Remember to execute all expensive operations (like time checks) outside the lock.
        self.state.lock().expect(ERR_POISONED_LOCK).exit(result, now, &self.options)
    }
}

#[derive(Debug)]
enum State {
    Closed { health: HealthMetrics },
    Open { open_until: Instant, stats: Stats },
    HalfOpen { probes: Probes, stats: Stats },
}

impl State {
    fn enter(&mut self, now: Instant, settings: &EngineOptions) -> EnterCircuitResult {
        match self {
            Self::Closed { .. } => EnterCircuitResult::Accepted {
                mode: ExecutionMode::Normal,
            },
            Self::Open { open_until, stats } => {
                if now >= *open_until {
                    let mut probes = Probes::new(&settings.probes);
                    let allow = probes.allow_probe(now);
                    stats.record_allow_result(allow);

                    *self = Self::HalfOpen {
                        probes,
                        stats: stats.clone(),
                    };
                    EnterCircuitResult::from(allow)
                } else {
                    stats.rejected = stats.rejected.saturating_add(1);
                    EnterCircuitResult::Rejected
                }
            }
            Self::HalfOpen { probes, stats: info } => {
                let allow = probes.allow_probe(now);
                info.record_allow_result(allow);
                EnterCircuitResult::from(allow)
            }
        }
    }

    fn exit(&mut self, result: ExecutionResult, now: Instant, settings: &EngineOptions) -> ExitCircuitResult {
        match self {
            Self::Closed { health } => {
                // first, record the result and evaluate the health metrics
                health.record(result, now);
                let health = health.health_info();

                // decide the next state based on health status
                match health.status() {
                    // Health is good, remain in a closed state
                    HealthStatus::Healthy => ExitCircuitResult::Unchanged,
                    // Health is poor, transition to Open state
                    HealthStatus::Unhealthy => {
                        *self = Self::Open {
                            open_until: now + settings.break_duration,
                            stats: Stats::new(now),
                        };
                        ExitCircuitResult::Opened(health)
                    }
                }
            }
            Self::Open { stats, .. } => {
                // Record lost results for statistics purposes
                stats.probes_lost = stats.probes_lost.saturating_add(1);

                // In open state, we don't process results. This can happen when multiple threads are involved and
                // the state of circuit breaker changes between enter and exit calls since these are separate
                // method calls that could be interleaved with other threads. Ignore the result.
                ExitCircuitResult::Unchanged
            }
            Self::HalfOpen { probes, stats } => {
                // record the result of the probe
                stats.record_probe_execution_result(result);

                match probes.record(result, now) {
                    ProbingResult::Success => {
                        let stats = stats.clone();

                        *self = Self::Closed {
                            health: settings.health_metrics_builder.build(),
                        };

                        ExitCircuitResult::Closed(stats)
                    }
                    ProbingResult::Failure => {
                        stats.re_opened = stats.re_opened.saturating_add(1);

                        *self = Self::Open {
                            open_until: now + settings.break_duration,
                            stats: stats.clone(),
                        };

                        ExitCircuitResult::Reopened
                    }
                    ProbingResult::Pending => ExitCircuitResult::Unchanged,
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Stats {
    pub opened_at: Instant,
    pub re_opened: usize,
    pub probes_total: usize,
    pub probes_lost: usize,
    pub probes_successes: usize,
    pub probes_failures: usize,
    pub rejected: usize,
}

impl Stats {
    pub fn new(opened_at: Instant) -> Self {
        Self {
            opened_at,
            probes_total: 0,
            probes_lost: 0,
            probes_successes: 0,
            probes_failures: 0,
            rejected: 0,
            re_opened: 0,
        }
    }

    pub fn opened_duration(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.opened_at)
    }

    fn record_allow_result(&mut self, allow: AllowProbeResult) {
        if allow == AllowProbeResult::Accepted {
            self.probes_total = self.probes_total.saturating_add(1);
        } else {
            self.rejected = self.rejected.saturating_add(1);
        }
    }

    fn record_probe_execution_result(&mut self, result: ExecutionResult) {
        match result {
            ExecutionResult::Success => {
                self.probes_successes = self.probes_successes.saturating_add(1);
            }
            ExecutionResult::Failure => {
                self.probes_failures = self.probes_failures.saturating_add(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Deref;

    use tick::ClockControl;

    use super::*;
    use crate::circuit_breaker::HealthMetricsBuilder;
    use crate::circuit_breaker::engine::probing::ProbesOptions;

    fn create_test_settings() -> EngineOptions {
        EngineOptions {
            break_duration: Duration::from_secs(5),
            health_metrics_builder: HealthMetricsBuilder::new(
                Duration::from_secs(30),
                0.1, // 10% failure threshold
                10,  // minimum 10 requests
            ),
            probes: ProbesOptions::quick(Duration::from_secs(2)),
        }
    }

    fn create_test_engine() -> EngineCore {
        let settings = create_test_settings();
        let clock = Clock::new_frozen();
        EngineCore::new(settings, clock)
    }

    fn open_engine(engine: &EngineCore) {
        const MAX_ATTEMPTS: usize = 1000;

        for _attempt in 0..MAX_ATTEMPTS {
            engine.enter();
            let result = engine.exit(ExecutionResult::Failure, ExecutionMode::Normal);
            if matches!(result, ExitCircuitResult::Opened(_)) {
                return;
            }
        }

        panic!("failed to open the circuit after {MAX_ATTEMPTS} attempts");
    }

    #[test]
    fn new_with_valid_settings_creates_closed_state() {
        let engine = create_test_engine();

        // Verify engine was created (we can't directly inspect the state due to encapsulation)
        // but we can verify it starts in closed state by checking enter() behavior
        let result = engine.enter();
        assert!(matches!(
            result,
            EnterCircuitResult::Accepted {
                mode: ExecutionMode::Normal
            }
        ));
    }

    #[test]
    fn enter_when_closed_accepts_request() {
        let engine = create_test_engine();

        let result = engine.enter();

        assert!(matches!(
            result,
            EnterCircuitResult::Accepted {
                mode: ExecutionMode::Normal
            }
        ));
    }

    #[test]
    fn enter_when_open_before_timeout_rejects_request() {
        let engine = create_test_engine();
        open_engine(&engine);

        // Verify circuit is now open
        let result = engine.enter();
        assert!(matches!(result, EnterCircuitResult::Rejected));
    }

    #[test]
    fn enter_when_open_after_timeout_transitions_to_half_open() {
        let settings = create_test_settings();
        let control = ClockControl::new();
        let clock = control.to_clock();
        let engine = EngineCore::new(settings, clock);

        // Force circuit to open
        open_engine(&engine);

        // Advance time beyond break duration
        control.advance(Duration::from_secs(6));

        let result = engine.enter();
        assert!(matches!(
            result,
            EnterCircuitResult::Accepted {
                mode: ExecutionMode::Probe
            }
        ));
    }

    #[test]
    fn enter_when_half_open_within_break_duration_rejects_request() {
        let settings = create_test_settings();
        let control = ClockControl::new();
        let clock = control.to_clock();
        let engine = EngineCore::new(settings, clock);

        // Force to open then half-open
        open_engine(&engine);
        control.advance(Duration::from_secs(6));
        engine.enter(); // Transitions to half-open

        // Try entering again immediately (within break duration)
        let result = engine.enter();
        assert!(matches!(result, EnterCircuitResult::Rejected));
    }

    #[test]
    fn enter_when_half_open_after_break_duration_resets_half_open_timer() {
        let settings = create_test_settings();
        let control = ClockControl::new();
        let clock = control.to_clock();
        let engine = EngineCore::new(settings, clock);

        // Force to open then half-open
        open_engine(&engine);
        control.advance(Duration::from_secs(6));
        engine.enter(); // Transitions to half-open

        // Advance time beyond break duration while in half-open
        control.advance(Duration::from_secs(6));

        let result = engine.enter();
        assert!(matches!(
            result,
            EnterCircuitResult::Accepted {
                mode: ExecutionMode::Probe
            }
        ));
    }

    #[test]
    fn exit_when_closed_with_success_remains_unchanged() {
        let engine = create_test_engine();
        engine.enter();

        let result = engine.exit(ExecutionResult::Success, ExecutionMode::Normal);

        assert!(matches!(result, ExitCircuitResult::Unchanged));
    }

    #[test]
    fn exit_when_closed_with_enough_failures_opens_circuit() {
        let settings = EngineOptions {
            break_duration: Duration::from_secs(5),
            health_metrics_builder: HealthMetricsBuilder::new(
                Duration::from_secs(30),
                0.1, // 10% failure threshold
                20,  // minimum 20 requests (higher than default 10 for this test)
            ),
            probes: ProbesOptions::quick(Duration::from_secs(2)),
        };
        let clock = Clock::new_frozen();
        let engine = EngineCore::new(settings, clock);

        // Record 19 successes and 3 failures = 22 total requests with ~13.6% failure rate
        for _ in 0..19 {
            engine.enter();
            engine.exit(ExecutionResult::Success, ExecutionMode::Normal);
        }
        for _ in 0..2 {
            engine.enter();
            engine.exit(ExecutionResult::Failure, ExecutionMode::Normal);
        }

        // One more failure to trigger opening: 3 failures out of 22 total = ~13.6% > 10%
        engine.enter();
        let result = engine.exit(ExecutionResult::Failure, ExecutionMode::Normal);

        assert!(matches!(result, ExitCircuitResult::Opened(_)));
    }

    #[test]
    fn exit_when_closed_with_insufficient_failures_remains_unchanged() {
        let engine = create_test_engine();

        // Record some failures but not enough to exceed a threshold (need at least 10 requests)
        for _ in 0..5 {
            engine.enter();
            engine.exit(ExecutionResult::Failure, ExecutionMode::Normal);
        }

        engine.enter();
        let result = engine.exit(ExecutionResult::Failure, ExecutionMode::Normal);

        assert!(matches!(result, ExitCircuitResult::Unchanged));
    }

    #[test]
    fn exit_when_open_ignores_result() {
        let engine = create_test_engine();
        open_engine(&engine);

        // Try to record success in open state
        let result = engine.exit(ExecutionResult::Success, ExecutionMode::Normal);
        assert!(matches!(result, ExitCircuitResult::Unchanged));

        if let State::Open { stats, .. } = engine.state.lock().unwrap().deref() {
            assert_eq!(stats.probes_lost, 1);
        } else {
            panic!("expected engine to be in Open state");
        }
    }

    #[test]
    fn exit_when_half_open_with_success_closes_circuit() {
        let settings = create_test_settings();
        let control = ClockControl::new();
        let clock = control.to_clock();
        let engine = EngineCore::new(settings, clock);

        // Force to open then half-open
        open_engine(&engine);
        control.advance(Duration::from_secs(6));
        engine.enter(); // Transitions to half-open

        let result = engine.exit(ExecutionResult::Success, ExecutionMode::Normal);

        assert!(matches!(result, ExitCircuitResult::Closed(stats) if stats.probes_successes == 1 && stats.probes_total == 1));
    }

    #[test]
    fn exit_when_half_open_with_failure_reopens_circuit() {
        let settings = create_test_settings();
        let control = ClockControl::new();
        let clock = control.to_clock();
        let engine = EngineCore::new(settings, clock);

        // Force to open then half-open
        open_engine(&engine);
        control.advance(Duration::from_secs(6));
        engine.enter(); // Transitions to half-open

        let result = engine.exit(ExecutionResult::Failure, ExecutionMode::Normal);

        assert!(matches!(result, ExitCircuitResult::Reopened));
    }

    #[test]
    fn circuit_breaker_full_cycle() {
        let settings = create_test_settings();
        let control = ClockControl::new();
        let clock = control.to_clock();
        let engine = EngineCore::new(settings, clock);

        // Start in closed state
        let result = engine.enter();
        assert!(matches!(
            result,
            EnterCircuitResult::Accepted {
                mode: ExecutionMode::Normal
            }
        ));

        // Force to open state
        open_engine(&engine);

        // Verify open state rejects requests
        let result = engine.enter();
        assert!(matches!(result, EnterCircuitResult::Rejected));

        // Advance time to allow transition to half-open
        control.advance(Duration::from_secs(6));
        let result = engine.enter();
        assert!(matches!(
            result,
            EnterCircuitResult::Accepted {
                mode: ExecutionMode::Probe
            }
        ));

        // Successful probe closes the circuit
        let result = engine.exit(ExecutionResult::Success, ExecutionMode::Normal);

        if let ExitCircuitResult::Closed(stats) = &result {
            assert_eq!(stats.probes_successes, 1);
            assert_eq!(stats.probes_total, 1);
            assert_eq!(stats.rejected, 1);
            assert_eq!(stats.probes_failures, 0);
            assert_eq!(stats.probes_lost, 0);
            assert_eq!(stats.re_opened, 0);
        } else {
            panic!("expected circuit to close after successful probe");
        }

        // Verify back to normal operation
        let result = engine.enter();
        assert!(matches!(
            result,
            EnterCircuitResult::Accepted {
                mode: ExecutionMode::Normal
            }
        ));
    }

    #[test]
    fn circuit_breaker_half_open_failure_cycle() {
        let settings = create_test_settings();
        let control = ClockControl::new();
        let clock = control.to_clock();
        let engine = EngineCore::new(settings, clock);

        // Force to open state
        open_engine(&engine);

        // Transition to half-open
        control.advance(Duration::from_secs(6));
        engine.enter();

        // Failed probe reopens circuit
        let result = engine.exit(ExecutionResult::Failure, ExecutionMode::Normal);
        assert!(matches!(result, ExitCircuitResult::Reopened));

        // Verify circuit is open again
        let result = engine.enter();
        assert!(matches!(result, EnterCircuitResult::Rejected));

        // Transition to half-open
        control.advance(Duration::from_secs(6));
        engine.enter();

        let result = engine.exit(ExecutionResult::Success, ExecutionMode::Normal);

        if let ExitCircuitResult::Closed(stats) = &result {
            assert_eq!(stats.probes_successes, 1);
            assert_eq!(stats.probes_total, 2);
            assert_eq!(stats.rejected, 1);
            assert_eq!(stats.probes_failures, 1);
            assert_eq!(stats.probes_lost, 0);
            assert_eq!(stats.re_opened, 1);
        } else {
            panic!("expected circuit to close after successful probe");
        }
    }

    #[test]
    fn concurrent_enter_exit_operations() {
        let engine = create_test_engine();

        // Simulate operations where enter and exit are called separately
        // (though each method call is atomic due to the internal mutex)
        engine.enter();
        let result1 = engine.exit(ExecutionResult::Success, ExecutionMode::Normal);

        engine.enter();
        let result2 = engine.exit(ExecutionResult::Failure, ExecutionMode::Normal);

        // Both should complete without panicking
        assert!(matches!(result1, ExitCircuitResult::Unchanged));
        assert!(matches!(result2, ExitCircuitResult::Unchanged));
    }

    #[test]
    fn engine_with_custom_break_duration() {
        let settings = EngineOptions {
            break_duration: Duration::from_millis(100),
            health_metrics_builder: HealthMetricsBuilder::new(Duration::from_secs(30), 0.1, 50),
            probes: ProbesOptions::quick(Duration::from_secs(2)),
        };
        let control = ClockControl::new();
        let clock = control.to_clock();
        let engine = EngineCore::new(settings, clock);

        // Force to open state
        open_engine(&engine);

        // Verify still rejected just before timeout
        control.advance(Duration::from_millis(99));
        let result = engine.enter();
        assert!(matches!(result, EnterCircuitResult::Rejected));

        // Verify accepted just after timeout
        control.advance(Duration::from_millis(2));
        let result = engine.enter();
        assert!(matches!(
            result,
            EnterCircuitResult::Accepted {
                mode: ExecutionMode::Probe
            }
        ));
    }

    #[test]
    fn engine_with_custom_failure_threshold() {
        let settings = EngineOptions {
            break_duration: Duration::from_secs(5),
            health_metrics_builder: HealthMetricsBuilder::new(
                Duration::from_secs(30),
                0.5, // 50% failure threshold
                10,  // minimum 10 requests
            ),
            probes: ProbesOptions::quick(Duration::from_secs(2)),
        };
        let control = ClockControl::new();
        let clock = control.to_clock();
        let engine = EngineCore::new(settings, clock);

        // Record 6 failures and 4 successes (60% failure rate, 10 total requests)
        for _ in 0..6 {
            engine.enter();
            engine.exit(ExecutionResult::Failure, ExecutionMode::Normal);
        }
        for _ in 0..3 {
            engine.enter();
            engine.exit(ExecutionResult::Success, ExecutionMode::Normal);
        }

        // Add one more failure to make it 7 failures out of 10 (70% > 50% threshold)
        engine.enter();
        let result = engine.exit(ExecutionResult::Failure, ExecutionMode::Normal);

        assert!(matches!(result, ExitCircuitResult::Opened(_)));
    }

    #[test]
    fn stats_record_probe_execution_result_increments_correctly() {
        let mut stats = Stats::new(Instant::now());

        stats.record_probe_execution_result(ExecutionResult::Success);
        assert_eq!(stats.probes_successes, 1);
        assert_eq!(stats.probes_failures, 0);

        stats.record_probe_execution_result(ExecutionResult::Failure);
        assert_eq!(stats.probes_successes, 1);
        assert_eq!(stats.probes_failures, 1);
    }

    #[test]
    fn stats_record_allow_result_increments_correctly() {
        let mut stats = Stats::new(Instant::now());

        stats.record_allow_result(AllowProbeResult::Accepted);
        assert_eq!(stats.probes_total, 1);
        assert_eq!(stats.rejected, 0);

        stats.record_allow_result(AllowProbeResult::Rejected);
        assert_eq!(stats.probes_total, 1);
        assert_eq!(stats.rejected, 1);
    }

    #[test]
    fn stats_opened_for_calculates_duration_correctly() {
        let opened_at = Instant::now();
        let stats = Stats::new(opened_at);

        // Simulate some time passing
        let later = opened_at + Duration::from_secs(10);

        assert_eq!(stats.opened_duration(later), Duration::from_secs(10));
    }
}
