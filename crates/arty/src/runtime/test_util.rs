// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Test-only runtime controls.

/// Runs `run` while runtime monitors use in-memory I/O.
///
/// The test still starts real runtime workers and monitoring request paths.
pub fn with_mock_monitor_io<T>(run: impl FnOnce() -> T) -> T {
    let (io, _control) = seismograph_protocol::io::MonitorIo::new_mocked();
    crate::runtime::monitoring::RuntimeMonitoring::with_test_io(io, run)
}
