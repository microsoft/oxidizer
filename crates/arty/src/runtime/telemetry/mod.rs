// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Event definitions and enrichment propagation; emission stays with behavior.

pub(crate) mod events;

/// The data class assigned to runtime telemetry fields.
///
/// Configure a sink's redaction policy for the `arty/SystemMetadata` class to
/// retain thread identifiers, names, and resource counts. Panic diagnostics use
/// the separate `arty/PanicMessage` class. Metric values themselves remain numeric.
pub(super) static SYSTEM_METADATA: data_privacy::DataClass = data_privacy::DataClass::new("arty", "SystemMetadata");
pub(super) static PANIC_MESSAGE: data_privacy::DataClass = data_privacy::DataClass::new("arty", "PanicMessage");
