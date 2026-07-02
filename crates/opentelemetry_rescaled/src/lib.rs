// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wraps an inner OpenTelemetry meter provider to transparently emit *rescaled*
//! side-by-side copies of selected instruments.
//!
//! For a chosen instrument in a chosen instrumentation scope, this layer creates
//! a second instrument whose measurements are the original values multiplied by a
//! fixed factor. For example, a `http.client.request.duration` instrument that
//! records seconds can gain a `http.client.request.duration.millis` sidecar that
//! records the same measurements multiplied by `1000.0`.
//!
//! The rescaling is invisible to instrument users — they interact only with their
//! original instrument — and the inner provider simply sees two independently
//! registered instruments.
//!
//! This crate is not yet implemented. See [`docs/DESIGN.md`](https://github.com/microsoft/oxidizer/blob/main/crates/opentelemetry_rescaled/docs/DESIGN.md)
//! for the architecture and open questions under review.
