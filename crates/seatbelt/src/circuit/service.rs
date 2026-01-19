// Copyright (c) Microsoft Corporation.

use std::ops::ControlFlow;

use layered::Service;
use tick::Clock;

use super::{
    CircuitLayer, CircuitEngine, Engines, EnterCircuitResult, ExecutionMode, ExecutionResult, ExitCircuitResult, OnClosed,
    OnClosedArgs, OnOpened, OnOpenedArgs, OnProbing, OnProbingArgs, PartionKeyProvider, PartitionKey, RecoveryArgs, RejectedInput,
    RejectedInputArgs, ShouldRecover,
};
use crate::{EnableIf, NotSet};

/// Applies circuit breaker logic to prevent cascading failures.
///
/// `Circuit` wraps an inner [`Service`] and monitors the success and failure rates
/// of operations. When the failure rate exceeds a configured threshold, the circuit breaker opens
/// and temporarily blocks requests to give the downstream service time to recover.
///
/// This middleware is designed to be used across services, applications, and libraries
/// to prevent cascading failures in distributed systems.
///
/// `Circuit` is configured by calling [`Circuit::layer`] and using the
/// builder methods on the returned [`CircuitLayer`] instance.
///
/// For comprehensive examples and usage patterns, see the [`circuit_breaker` module][crate::circuit] documentation.
#[derive(Debug)]
pub struct Circuit<In, Out, S> {
    pub(super) inner: S,
    pub(super) clock: Clock,
    pub(super) recovery: ShouldRecover<Out>,
    pub(super) rejected_input: RejectedInput<In, Out>,
    pub(super) enable_if: EnableIf<In>,
    pub(super) engines: Engines,
    pub(super) partition_key: Option<PartionKeyProvider<In>>,
    pub(super) on_opened: Option<OnOpened<Out>>,
    pub(super) on_closed: Option<OnClosed<Out>>,
    pub(super) on_probing: Option<OnProbing<In>>,
}

impl<In, Out> Circuit<In, Out, ()> {
    /// Creates a new circuit breaker layer with the specified name and options.
    ///
    /// Returns a [`CircuitLayer`] that must be configured with required parameters
    /// before it can be used to build a circuit breaker service.
    pub fn layer(
        name: impl Into<std::borrow::Cow<'static, str>>,
        options: &crate::SeatbeltOptions<In, Out>,
    ) -> CircuitLayer<In, Out, NotSet, NotSet> {
        CircuitLayer::new(name.into().into(), options)
    }
}

impl<In, Out: Send, S> Service<In> for Circuit<In, Out, S>
where
    In: Send,
    S: Service<In, Out = Out>,
{
    type Out = Out;

    async fn execute(&self, input: In) -> Self::Out {
        // Check if a circuit breaker is enabled for this input
        if !self.enable_if.call(&input) {
            return self.inner.execute(input).await;
        }

        // Determine the partition key for this input
        let partition_key = self
            .partition_key
            .as_ref()
            .map_or_else(PartitionKey::default, |partition_key| partition_key.call(&input));

        // Retrieve the engine for this partition
        let engine = self.engines.get_engine(&partition_key);

        // Before
        let (input, mode) = match self.before_execute(engine.as_ref(), input, &partition_key) {
            ControlFlow::Continue(input) => input,
            ControlFlow::Break(output) => return output,
        };

        // Execute the inner service
        let output = self.inner.execute(input).await;

        // After
        self.after_execute(engine.as_ref(), &output, mode, &partition_key);

        output
    }
}

impl<In, Out, S> Circuit<In, Out, S> {
    #[inline]
    fn before_execute(
        &self,
        engine: &impl CircuitEngine,
        mut input: In,
        partition_key: &PartitionKey,
    ) -> ControlFlow<Out, (In, ExecutionMode)> {
        // Try to enter the circuit
        match engine.enter() {
            EnterCircuitResult::Accepted { mode } => {
                match mode {
                    // regular execution, do nothing special
                    ExecutionMode::Normal => ControlFlow::Continue((input, ExecutionMode::Normal)),
                    // This is a probing execution that happens when the circuit is half-open.
                    // Invoke the on_probing callback if configured.
                    ExecutionMode::Probe => {
                        if let Some(on_probing) = &self.on_probing {
                            on_probing.call(&mut input, OnProbingArgs { partition_key });
                        }

                        ControlFlow::Continue((input, ExecutionMode::Probe))
                    }
                }
            }
            // Circuit is open, return rejected input output
            EnterCircuitResult::Rejected => ControlFlow::Break(self.rejected_input.call(input, RejectedInputArgs { partition_key })),
        }
    }

    fn after_execute(&self, engine: &impl CircuitEngine, output: &Out, mode: ExecutionMode, partition_key: &PartitionKey) {
        let recovery = self.recovery.call(
            output,
            RecoveryArgs {
                partition_key,
                clock: &self.clock,
            },
        );

        // Evaluate the execution result based on recovery decision
        let execution_result = ExecutionResult::from_recovery(&recovery);

        // Exit the circuit and handle state transitions
        match engine.exit(execution_result, mode) {
            ExitCircuitResult::Unchanged | ExitCircuitResult::Reopened => {
                // we explicitly do nothing here
            }
            ExitCircuitResult::Opened(_health) => {
                if let Some(on_opened) = &self.on_opened {
                    on_opened.call(output, OnOpenedArgs { partition_key });
                }
            }
            ExitCircuitResult::Closed(stats) => {
                if let Some(on_closed) = &self.on_closed {
                    on_closed.call(
                        output,
                        OnClosedArgs {
                            partition_key,
                            open_duration: stats.opened_duration(self.clock.instant()),
                        },
                    );
                }
            }
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
#[cfg(not(miri))]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use layered::Execute;
    use tick::ClockControl;

    use super::*;
    use crate::circuit::constants::DEFAULT_BREAK_DURATION;
    use crate::circuit::{EngineFake, HalfOpenMode, HealthInfo, Stats};
    use crate::service::Layer;
    use crate::{RecoveryInfo, SeatbeltOptions, Set};

    #[test]
    fn layer_ensure_defaults() {
        let options = SeatbeltOptions::<String, String>::new(Clock::new_frozen()).pipeline_name("test_pipeline");
        let layer: CircuitLayer<String, String, NotSet, NotSet> = Circuit::layer("test_breaker", &options);
        let layer = layer
            .recovery_with(|_, _| RecoveryInfo::never())
            .rejected_input(|_, _| "rejected".to_string());

        let breaker = layer.layer(Execute::new(|v: String| async move { v }));

        assert!(breaker.enable_if.call(&"str".to_string()));
    }

    #[tokio::test]
    async fn circuit_breaker_disabled_no_inner_calls() {
        let clock = Clock::new_frozen();
        let service = create_ready_circuit_breaker_layer(&clock)
            .disable()
            .layer(Execute::new(move |v: String| async move { v }));

        let result = service.execute("test".to_string()).await;

        assert_eq!(result, "test");
    }

    #[tokio::test]
    async fn passthrough_behavior() {
        let clock = Clock::new_frozen();
        let service = create_ready_circuit_breaker_layer(&clock).layer(Execute::new(move |v: String| async move { v }));

        let result = service.execute("test".to_string()).await;

        assert_eq!(result, "test");
    }

    #[test]
    fn before_execute_accepted() {
        let service = create_ready_circuit_breaker_layer(&Clock::new_frozen())
            .on_probing(|_, _| panic!("should not be called"))
            .layer(Execute::new(move |v: String| async move { v }));

        let engine = EngineFake::new(
            EnterCircuitResult::Accepted {
                mode: ExecutionMode::Normal,
            },
            ExitCircuitResult::Unchanged,
        );

        let result = service
            .before_execute(&engine, "test".to_string(), &PartitionKey::default())
            .continue_value()
            .unwrap();
        assert_eq!(result, ("test".to_string(), ExecutionMode::Normal));
    }

    #[test]
    fn before_execute_accepted_with_probing() {
        let probing_called = Arc::new(AtomicBool::new(false));
        let probing_called_clone = Arc::clone(&probing_called);

        let service = create_ready_circuit_breaker_layer(&Clock::new_frozen())
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
            .before_execute(&engine, "test".to_string(), &PartitionKey::default())
            .continue_value()
            .unwrap();
        assert_eq!(result, ("test".to_string(), ExecutionMode::Probe));
        assert!(probing_called_clone.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn before_execute_rejected() {
        let service = create_ready_circuit_breaker_layer(&Clock::new_frozen())
            .rejected_input(|_, _| "rejected".to_string())
            .layer(Execute::new(move |v: String| async move { v }));

        let engine = EngineFake::new(EnterCircuitResult::Rejected, ExitCircuitResult::Unchanged);

        let result = service
            .before_execute(&engine, "test".to_string(), &PartitionKey::default())
            .break_value()
            .unwrap();
        assert_eq!(result, "rejected");
    }

    #[test]
    fn after_execute_unchanged() {
        let service = create_ready_circuit_breaker_layer(&Clock::new_frozen())
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
        service.after_execute(&engine, &"success".to_string(), ExecutionMode::Normal, &PartitionKey::default());
    }

    #[test]
    fn after_execute_reopened() {
        let service = create_ready_circuit_breaker_layer(&Clock::new_frozen())
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
        service.after_execute(&engine, &"success".to_string(), ExecutionMode::Normal, &PartitionKey::default());
    }

    #[test]
    fn after_execute_opened() {
        let opened_called = Arc::new(AtomicBool::new(false));
        let opened_called_clone = Arc::clone(&opened_called);

        let service = create_ready_circuit_breaker_layer(&Clock::new_frozen())
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

        service.after_execute(
            &engine,
            &"error_response".to_string(),
            ExecutionMode::Normal,
            &PartitionKey::default(),
        );
        assert!(opened_called_clone.load(Ordering::SeqCst));
    }

    #[test]
    fn after_execute_closed() {
        let closed_called = Arc::new(AtomicBool::new(false));
        let closed_called_clone = Arc::clone(&closed_called);

        let service = create_ready_circuit_breaker_layer(&Clock::new_frozen())
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

        service.after_execute(
            &engine,
            &"success_response".to_string(),
            ExecutionMode::Normal,
            &PartitionKey::default(),
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
        let service = create_ready_circuit_breaker_layer(&clock_control.to_clock())
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
        let service = create_ready_circuit_breaker_layer(&clock)
            .partition_key(|input| PartitionKey::from(input.clone()))
            .min_throughput(3)
            .recovery_with(|_, _| RecoveryInfo::retry())
            .rejected_input(|_, args| format!("circuit is open, partition: {}", args.partition_key))
            .layer(Execute::new(|input: String| async move { input }));

        // break the circuit for partition "A"
        for _ in 0..3 {
            let result = service.execute("A".to_string()).await;
            assert_eq!(result, "A");
        }

        let result = service.execute("A".to_string()).await;
        assert_eq!(result, "circuit is open, partition: A");

        // Execute on partition "B" should pass through
        let result = service.execute("B".to_string()).await;
        assert_eq!(result, "B");
    }

    fn create_ready_circuit_breaker_layer(clock: &Clock) -> CircuitLayer<String, String, Set, Set> {
        let options = SeatbeltOptions::<String, String>::new(clock.clone()).pipeline_name("test_pipeline");
        Circuit::layer("test_breaker", &options)
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
