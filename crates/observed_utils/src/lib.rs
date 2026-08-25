// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/observed_utils/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/observed_utils/favicon.ico")]
// TODO(doc-coverage): remove once `missing_docs` is promoted to [workspace.lints.rust].
#![deny(missing_docs)]

//! Helper types and utilities that expand `observed` functionality.
//!
//! [`observed`] itself does not depend on `opentelemetry`: an event is a typed
//! struct, and a crate that only emits events should not compile an exporter's
//! dependency tree. A crate that *does* export to OpenTelemetry needs the
//! conversion anyway, so it lives here - once, rather than re-derived in every
//! exporter.
//!
//! - [`any_value_of`], [`otel_value_of`] and [`otel_severity_of`] convert an
//!   [`observed::Value`] or [`observed::Severity`] into its OpenTelemetry
//!   counterpart. OpenTelemetry has no unsigned value, so a `u64` converts to an
//!   `i64` saturating at `i64::MAX` rather than wrapping.
//! - [`metric_number_of`] extracts the number a metric instrument records.
//! - [`format_any_value`] renders an [`AnyValue`](opentelemetry::logs::AnyValue)
//!   in human-readable form instead of its `Debug` shape.
//! - [`SensitiveSlice`] holds a bounded, type-erased collection of classified
//!   items inline, without allocating for the collection, and renders them
//!   through the caller's redactor.
//!
//! This crate is less stable than `observed` itself and may have breaking changes.

mod format_any_value;
mod metric_number;
mod otel;
mod sensitive_slice;

pub use format_any_value::format_any_value;
pub use metric_number::metric_number_of;
pub use otel::{any_value_of, otel_severity_of, otel_value_of};
pub use sensitive_slice::SensitiveSlice;
