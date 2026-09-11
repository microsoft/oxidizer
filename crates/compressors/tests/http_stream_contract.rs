// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Contract stubs for HTTP-facing compression stream behavior.

#[test]
#[ignore = "contract stub"]
fn source_error_can_be_taken_by_value() {
    // Wrap a typed source error, extract it by value, and assert its concrete type is preserved.
}

#[test]
#[ignore = "contract stub"]
fn compression_stream_recovers_source() {
    // Build a compression stream, consume the adapter, and assert the original source is returned.
}

#[test]
#[ignore = "contract stub"]
fn compression_stream_flushes_one_idle_burst() {
    // Feed one burst, return Pending, and assert output is flushed exactly once before new input.
}
