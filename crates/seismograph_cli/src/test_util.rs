// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! In-memory access to the production snapshot decoding and rendering pipeline.

/// Decodes snapshot bytes and renders HTML without filesystem or process I/O.
///
/// # Errors
///
/// Returns the snapshot decoding error.
pub fn render_capture(bytes: &[u8]) -> Result<String, Box<dyn std::error::Error>> {
    crate::commands::snapshot::html::render_capture(bytes).map_err(Into::into)
}
