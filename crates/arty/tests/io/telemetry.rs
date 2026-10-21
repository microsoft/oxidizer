// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! I/O driver telemetry behavior.

use crate::*;

#[test]
fn successful_initialization_reports_context_and_role_telemetry() {
    let (runtime, processor) = runtime_with_telemetry(1);
    let scenario = Scenario::options(RolePlan::PreferPrimary);
    runtime
        .scheduler()
        .block_on({
            async move |builtins| {
                let _ = builtins.io().context::<TestContext<CacheContext>>(&scenario);
            }
        })
        .unwrap();
    runtime.stop().unwrap();

    let events: Vec<CapturedEvent> = processor
        .events()
        .into_iter()
        .filter(|event| event.name() == "arty.rt.io.driver.initialized")
        .collect();
    assert_eq!(events.len(), 1);
    let dimensions = events[0].dimensions();
    assert!(
        dimensions
            .iter()
            .any(|(name, value)| name == "io.context" && value.to_string().contains("CacheContext"))
    );
    assert!(
        dimensions
            .iter()
            .any(|(name, value)| name == "io.driver.role" && *value == "primary".into())
    );
}

#[test]
fn initialization_failure_reports_a_diagnostic_message() {
    let (runtime, processor) = runtime_with_telemetry(1);
    let worker = worker_schedulers(&runtime, 1).pop().unwrap();
    let scenario = Scenario::options(RolePlan::PreferPrimary);
    scenario.fail_driver_at.store(1, Ordering::SeqCst);

    worker
        .spawn({
            async move |builtins| {
                let _ = builtins.io().try_context::<TestContext<ConstructionFailure>>(&scenario);
            }
        })
        .join()
        .unwrap();
    runtime.stop().unwrap_err();

    let event = processor
        .events()
        .into_iter()
        .find(|event| event.name() == "arty.rt.io.driver.initialization_failed")
        .unwrap();
    assert!(
        event
            .dimensions()
            .iter()
            .any(|(name, value)| name == "error.message" && *value == "driver construction failed".into())
    );
}
