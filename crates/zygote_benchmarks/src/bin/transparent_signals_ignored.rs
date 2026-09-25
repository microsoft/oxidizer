// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Transparent benchmark fixture with ignored signals.

#[cfg(target_os = "linux")]
mod startup;

include!("transparent.rs");
