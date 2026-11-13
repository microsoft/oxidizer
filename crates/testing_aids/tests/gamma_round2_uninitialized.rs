// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This target deliberately has no initializer: helpers must reject capture before initialization.

#[test]
#[should_panic(expected = "testing_aids tracing was used before initialize() ran")]
fn gamma_round2_thread_local_capture_requires_initialization() {
    let capture = testing_aids::tracing_logs::Capture::new();
    drop(capture.subscriber());
}

#[test]
#[should_panic(expected = "testing_aids tracing was used before initialize() ran")]
fn gamma_round2_process_buffer_requires_initialization() {
    drop(testing_aids::tracing_logs::write_to_stdout_and_buffer());
}
