// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Event definitions and enrichment propagation; emission stays with behavior.

pub(crate) mod enrichment;
pub(crate) mod events;

/// The data class assigned to runtime telemetry fields.
///
/// Configure a sink's redaction policy for the `arty/SystemMetadata` class to
/// retain these fields. Runtime telemetry includes thread identifiers, names,
/// resource counts, and panic diagnostics. Metric values themselves remain numeric.
pub(super) static SYSTEM_METADATA: data_privacy::DataClass = data_privacy::DataClass::new("arty", "SystemMetadata");
