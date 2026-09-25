// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Transparent benchmark fixture with custom signal handlers.

#[cfg(target_os = "linux")]
mod startup;

include!("transparent.rs");
