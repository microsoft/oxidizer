// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Transparent benchmark fixture using the descriptor-cleanup fallback.

#[cfg(target_os = "linux")]
mod startup;

include!("transparent.rs");
