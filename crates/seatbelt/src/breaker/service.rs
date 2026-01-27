// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ops::ControlFlow;
use std::sync::Arc;

use layered::Service;
use tick::Clock;

use super::*;
use crate::{NotSet, utils::EnableIf};

/// Applies circuit breaker logic to prevent cascading failures.
///
/// `Breaker` wraps an inner [`Service`] and monitors the success and failure rates
/// of operations. When the failure rate exceeds a configured threshold, the circuit breaker opens
/// and temporarily blocks inputs to give the downstream service time to recover.
///
/// This middleware is designed to be used across services, applications, and libraries
/// to prevent cascading failures in distributed systems.
///
/// `Breaker` is configured by calling [`Breaker::layer`] and using the
/// builder methods on the returned [`BreakerLayer`] instance.
///
/// For comprehensive examples and usage patterns, see the [`breaker` module][crate::breaker] documentation.
pub struct Breaker<In, Out, S> {
    pub(super) shared: Arc<BreakerShared<In, Out>>,
    pub(super) inner: S,
}

/// Shared configuration for [`Breaker`] middleware.
///
/// This struct is wrapped in an `Arc` to enable cheap cloning of the service.
pub(crate) struct BreakerShared<In, Out> {
    pub(crate) clock: Clock,
    pub(crate) recovery: ShouldRecover<Out>,
    pub(crate) rejected_input: RejectedInput<In, Out>,
    pub(crate) enable_if: EnableIf<In>,
    pub(crate) engines: Engines,
    pub(crate) id_provider: Option<BreakerIdProvider<In>>,
    pub(crate) on_opened: Option<OnOpened<Out>>,
    pub(crate) on_closed: Option<OnClosed<Out>>,
    pub(crate) on_probing: Option<OnProbing<In>>,
}

impl<In, Out, S: Clone> Clone for Breaker<In, Out, S> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
            inner: self.inner.clone(),
        }
    }
}

impl<In, Out, S: std::fmt::Debug> std::fmt::Debug for Breaker<In, Out, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Breaker").field("inner", &self.inner).finish_non_exhaustive()
    }
}

impl<In, Out> Breaker<In, Out, ()> {
    /// Creates a new circuit breaker layer with the specified name and options.
    ///
    /// Returns a [`BreakerLayer`] that must be configured with required parameters
    /// before it can be used to build a circuit breaker service.
    pub fn layer(
        name: impl Into<std::borrow::Cow<'static, str>>,
        context: &crate::ResilienceContext<In, Out>,
    ) -> BreakerLayer<In, Out, NotSet, NotSet> {
        BreakerLayer::new(name.into(), context)
    }
}

impl<In, Out: Send, S> Service<In> for Breaker<In, Out, S>
where
    In: Send,
    S: Service<In, Out = Out>,
{
    type Out = Out;

    async fn execute(&self, input: In) -> Self::Out {
        if !self.shared.enable_if.call(&input) {
            return self.inner.execute(input).await;
        }

        let breaker_id = self.shared.get_breaker_id(&input);
        let engine = self.shared.engines.get_engine(&breaker_id);

        let (input, mode) = match self.shared.before_execute(engine.as_ref(), input, &breaker_id) {
            ControlFlow::Continue(result) => result,
            ControlFlow::Break(output) => return output,
        };

        let output = self.inner.execute(input).await;

        self.shared.after_execute(engine.as_ref(), &output, mode, &breaker_id);

        output
    }
}

impl<In, Out> BreakerShared<In, Out> {
    fn get_breaker_id(&self, input: &In) -> BreakerId {
        self.id_provider
            .as_ref()
            .map_or_else(BreakerId::default, |provider| provider.call(input))
    }

    fn before_execute(&self, engine: &impl CircuitEngine, mut input: In, breaker_id: &BreakerId) -> ControlFlow<Out, (In, ExecutionMode)> {
        match engine.enter() {
            EnterCircuitResult::Accepted { mode } => {
                if mode == ExecutionMode::Probe {
                    self.invoke_on_probing(&mut input, breaker_id);
                }
                ControlFlow::Continue((input, mode))
            }
            EnterCircuitResult::Rejected => ControlFlow::Break(self.rejected_input.call(input, RejectedInputArgs { breaker_id })),
        }
    }

    fn after_execute(&self, engine: &impl CircuitEngine, output: &Out, mode: ExecutionMode, breaker_id: &BreakerId) {
        let recovery = self.recovery.call(
            output,
            RecoveryArgs {
                breaker_id,
                clock: &self.clock,
            },
        );

        let execution_result = ExecutionResult::from_recovery(&recovery);

        match engine.exit(execution_result, mode) {
            ExitCircuitResult::Unchanged | ExitCircuitResult::Reopened => {}
            ExitCircuitResult::Opened(_health) => {
                self.invoke_on_opened(output, breaker_id);
            }
            ExitCircuitResult::Closed(stats) => {
                self.invoke_on_closed(output, breaker_id, stats.opened_duration(self.clock.instant()));
            }
        }
    }

    fn invoke_on_probing(&self, input: &mut In, breaker_id: &BreakerId) {
        if let Some(on_probing) = &self.on_probing {
            on_probing.call(input, OnProbingArgs { breaker_id });
        }
    }

    fn invoke_on_opened(&self, output: &Out, breaker_id: &BreakerId) {
        if let Some(on_opened) = &self.on_opened {
            on_opened.call(output, OnOpenedArgs { breaker_id });
        }
    }

    fn invoke_on_closed(&self, output: &Out, breaker_id: &BreakerId, open_duration: std::time::Duration) {
        if let Some(on_closed) = &self.on_closed {
            on_closed.call(output, OnClosedArgs { breaker_id, open_duration });
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
#[cfg(not(miri))]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use layered::Execute;
    use tick::ClockControl;

    use super::*;
    use crate::breaker::constants::DEFAULT_BREAK_DURATION;
    use crate::{RecoveryInfo, ResilienceContext, Set};
    use layered::Layer;

    #[test]
    fn layer_ensure_defaults() {
        let context = ResilienceContext::<String, String>::new(Clock::new_frozen()).name("test_pipeline");
        let layer: BreakerLayer<String, String, NotSet, NotSet> = Breaker::layer("test_breaker", &context);
        let layer = layer
            .recovery_with(|_, _| RecoveryInfo::never())
            .rejected_input(|_, _| "rejected".to_string());

        let breaker = layer.layer(Execute::new(|v: String| async move { v }));

        assert!(breaker.shared.enable_if.call(&"str".to_string()));
    }

    #[tokio::test]
    async fn breaker_disabled_no_inner_calls() {
        let clock = Clock::new_frozen();
        let service = create_ready_breaker_layer(&clock)
            .disable()
            .layer(Execute::new(move |v: String| async move { v }));

        let result = service.execute("test".to_string()).await;

        assert_eq!(result, "test");
    }

    #[tokio::test]
    async fn passthrough_behavior() {
        let clock = Clock::new_frozen();
        let service = create_ready_breaker_layer(&clock).layer(Execute::new(move |v: String| async move { v }));

        let result = service.execute("test".to_string()).await;

        assert_eq!(result, "test");
    }

    #[test]
    fn before_execute_accepted() {
        let service = create_ready_breaker_layer(&Clock::new_frozen())
            .on_probing(|_, _| panic!("should not be called"))
            .layer(Execute::new(move |v: String| async move { v }));

        let engine = EngineFake::new(
            EnterCircuitResult::Accepted {
                mode: ExecutionMode::Normal,
            },
            ExitCircuitResult::Unchanged,
        );

        let result = service
            .shared
            .before_execute(&engine, "test".to_string(), &BreakerId::default())
            .continue_value()
            .unwrap();
        assert_eq!(result, ("test".to_string(), ExecutionMode::Normal));
    }

    #[test]
    fn before_execute_accepted_with_probing() {
        let probing_called = Arc::new(AtomicBool::new(false));
        let probing_called_clone = Arc::clone(&probing_called);

        let service = create_ready_breaker_layer(&Clock::new_frozen())
            .on_probing(move |value, _| {
                assert_eq!(value, "test");
                probing_called.store(true, std::sync::atomic::Ordering::SeqCst);
            })
            .layer(Execute::new(move |v: String| async move { v }));

        let engine = EngineFake::new(
            EnterCircuitResult::Accepted {
                mode: ExecutionMode::Probe,
            },
            ExitCircuitResult::Unchanged,
        );

        let result = service
            .shared
            .before_execute(&engine, "test".to_string(), &BreakerId::default())
            .continue_value()
            .unwrap();
        assert_eq!(result, ("test".to_string(), ExecutionMode::Probe));
        assert!(probing_called_clone.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn before_execute_rejected() {
        let service = create_ready_breaker_layer(&Clock::new_frozen())
            .rejected_input(|_, _| "rejected".to_string())
            .layer(Execute::new(move |v: String| async move { v }));

        let engine = EngineFake::new(EnterCircuitResult::Rejected, ExitCircuitResult::Unchanged);

        let result = service
            .shared
            .before_execute(&engine, "test".to_string(), &BreakerId::default())
            .break_value()
            .unwrap();
        assert_eq!(result, "rejected");
    }

    #[test]
    fn after_execute_unchanged() {
        let service = create_ready_breaker_layer(&Clock::new_frozen())
            .on_opened(|_, _| panic!("should not be called"))
            .on_closed(|_, _| panic!("should not be called"))
            .layer(Execute::new(move |v: String| async move { v }));

        let engine = EngineFake::new(
            EnterCircuitResult::Accepted {
                mode: ExecutionMode::Normal,
            },
            ExitCircuitResult::Unchanged,
        );

        // This should not panic, indicating no callbacks were invoked
        service
            .shared
            .after_execute(&engine, &"success".to_string(), ExecutionMode::Normal, &BreakerId::default());
    }

    #[test]
    fn after_execute_reopened() {
        let service = create_ready_breaker_layer(&Clock::new_frozen())
            .on_opened(|_, _| panic!("should not be called"))
            .on_closed(|_, _| panic!("should not be called"))
            .layer(Execute::new(move |v: String| async move { v }));

        let engine = EngineFake::new(
            EnterCircuitResult::Accepted {
                mode: ExecutionMode::Normal,
            },
            ExitCircuitResult::Reopened,
        );

        // This should not panic, indicating no callbacks were invoked
        service
            .shared
            .after_execute(&engine, &"success".to_string(), ExecutionMode::Normal, &BreakerId::default());
    }

    #[test]
    fn after_execute_opened() {
        let opened_called = Arc::new(AtomicBool::new(false));
        let opened_called_clone = Arc::clone(&opened_called);

        let service = create_ready_breaker_layer(&Clock::new_frozen())
            .on_opened(move |output, _| {
                assert_eq!(output, "error_response");
                opened_called.store(true, Ordering::SeqCst);
            })
            .on_closed(|_, _| panic!("on_closed should not be called"))
            .layer(Execute::new(move |v: String| async move { v }));

        let engine = EngineFake::new(
            EnterCircuitResult::Accepted {
                mode: ExecutionMode::Normal,
            },
            ExitCircuitResult::Opened(HealthInfo::new(1, 1, 1.0, 1)),
        );

        service
            .shared
            .after_execute(&engine, &"error_response".to_string(), ExecutionMode::Normal, &BreakerId::default());
        assert!(opened_called_clone.load(Ordering::SeqCst));
    }

    #[test]
    fn after_execute_closed() {
        let closed_called = Arc::new(AtomicBool::new(false));
        let closed_called_clone = Arc::clone(&closed_called);

        let service = create_ready_breaker_layer(&Clock::new_frozen())
            .on_opened(|_, _| panic!("on_opened should not be called"))
            .on_closed(move |output, _| {
                assert_eq!(output, "success_response");
                closed_called.store(true, Ordering::SeqCst);
            })
            .layer(Execute::new(move |v: String| async move { v }));

        let engine = EngineFake::new(
            EnterCircuitResult::Accepted {
                mode: ExecutionMode::Normal,
            },
            ExitCircuitResult::Closed(Stats::new(Instant::now())),
        );

        service.shared.after_execute(
            &engine,
            &"success_response".to_string(),
            ExecutionMode::Normal,
            &BreakerId::default(),
        );
        assert!(closed_called_clone.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn execute_end_to_end_with_callbacks() {
        let probing_called = Arc::new(AtomicBool::new(false));
        let opened_called = Arc::new(AtomicBool::new(false));
        let closed_called = Arc::new(AtomicBool::new(false));

        let probing_called_clone = Arc::clone(&probing_called);
        let opened_called_clone = Arc::clone(&opened_called);
        let closed_called_clone = Arc::clone(&closed_called);

        let clock_control = ClockControl::new();

        // Create a service that transforms input and can trigger different circuit states
        let service = create_ready_breaker_layer(&clock_control.to_clock())
            .min_throughput(5)
            .half_open_mode(HalfOpenMode::quick())
            .on_probing(move |input, _| {
                assert_eq!(input, "probe_input");
                probing_called.store(true, Ordering::SeqCst);
            })
            .on_opened(move |output, _| {
                assert_eq!(output, "error_output");
                opened_called.store(true, Ordering::SeqCst);
            })
            .on_closed(move |output, args| {
                assert_eq!(output, "probe_output");
                assert!(args.open_duration() > Duration::ZERO);
                closed_called.store(true, Ordering::SeqCst);
            })
            .layer(Execute::new(move |input: String| async move {
                // Transform input to simulate different scenarios
                match input.as_str() {
                    "probe_input" => "probe_output".to_string(),
                    "success_input" => "success_output".to_string(),
                    "error_input" => "error_output".to_string(),
                    _ => input,
                }
            }));

        // break the circuit first by simulating failures
        for _ in 0..5 {
            let result = service.execute("error_input".to_string()).await;
            assert_eq!(result, "error_output");
        }

        // rejected input
        let result = service.execute("success_input".to_string()).await;
        assert_eq!(result, "circuit is open");
        assert!(opened_called_clone.load(Ordering::SeqCst));
        assert!(!closed_called_clone.load(Ordering::SeqCst));

        // send probe and close the circuit
        clock_control.advance(DEFAULT_BREAK_DURATION);
        let result = service.execute("probe_input".to_string()).await;
        assert_eq!(result, "probe_output");
        assert!(probing_called_clone.load(Ordering::SeqCst));
        assert!(closed_called_clone.load(Ordering::SeqCst));

        // normal execution should pass through
        let result = service.execute("success_input".to_string()).await;
        assert_eq!(result, "success_output");
    }

    #[tokio::test]
    async fn different_partitions_ensure_isolated() {
        let clock = Clock::new_frozen();
        let service = create_ready_breaker_layer(&clock)
            .breaker_id(|input| BreakerId::from(input.clone()))
            .min_throughput(3)
            .recovery_with(|_, _| RecoveryInfo::retry())
            .rejected_input(|_, args| format!("circuit is open, breaker: {}", args.breaker_id()))
            .layer(Execute::new(|input: String| async move { input }));

        // break the circuit for partition "A"
        for _ in 0..3 {
            let result = service.execute("A".to_string()).await;
            assert_eq!(result, "A");
        }

        let result = service.execute("A".to_string()).await;
        assert_eq!(result, "circuit is open, breaker: A");

        // Execute on partition "B" should pass through
        let result = service.execute("B".to_string()).await;
        assert_eq!(result, "B");
    }

    #[tokio::test]
    async fn breaker_emits_logs() {
        use tracing_subscriber::util::SubscriberInitExt;

        use crate::testing::LogCapture;

        let log_capture = LogCapture::new();
        let _guard = log_capture.subscriber().set_default();

        let clock_control = ClockControl::new();
        let context = ResilienceContext::<String, String>::new(clock_control.to_clock())
            .name("log_test_pipeline")
            .use_logs();

        let service = Breaker::layer("log_test_circuit", &context)
            .min_throughput(3)
            .half_open_mode(HalfOpenMode::quick())
            .recovery_with(|output, _| {
                if output.contains("success") {
                    RecoveryInfo::never()
                } else {
                    RecoveryInfo::retry()
                }
            })
            .rejected_input(|_, _| "rejected".to_string())
            .layer(Execute::new(|input: String| async move { input }));

        // Trip the circuit by generating failures
        for _ in 0..3 {
            let _ = service.execute("fail".to_string()).await;
        }

        // Verify circuit opened log
        log_capture.assert_contains("seatbelt::breaker");
        log_capture.assert_contains("log_test_pipeline");
        log_capture.assert_contains("log_test_circuit");
        log_capture.assert_contains("circuit_breaker.state=\"open\"");
        log_capture.assert_contains("circuit_breaker.health.failure_rate");

        // Request should be rejected (emits another open state log)
        let _ = service.execute("test".to_string()).await;

        // Advance time past break duration to allow probing
        clock_control.advance(DEFAULT_BREAK_DURATION);

        // Send a successful probe to close circuit
        let _ = service.execute("success".to_string()).await;
        log_capture.assert_contains("circuit_breaker.probe.result");
        log_capture.assert_contains("circuit_breaker.state=\"closed\"");
        log_capture.assert_contains("circuit_breaker.open.duration");
    }

    fn create_ready_breaker_layer(clock: &Clock) -> BreakerLayer<String, String, Set, Set> {
        let context = ResilienceContext::<String, String>::new(clock.clone()).name("test_pipeline");
        Breaker::layer("test_breaker", &context)
            .recovery_with(|output, _| {
                if output.contains("error") {
                    RecoveryInfo::retry()
                } else {
                    RecoveryInfo::never()
                }
            })
            .rejected_input(|_, _| "circuit is open".to_string())
    }
}
