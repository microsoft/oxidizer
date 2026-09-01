// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// How a single-stream decoder handles bytes after the compressed stream.
///
/// In multi-stream mode, subsequent bytes are always interpreted as another compressed stream and
/// must be valid. This policy applies when multi-stream decoding is disabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum TrailingData {
    /// Stop after the compressed stream and preserve already-buffered trailing bytes.
    ///
    /// Retrieve them with the decoder's `take_remainder` method.
    #[default]
    Preserve,

    /// Require the compressed stream to end exactly at end of input.
    ///
    /// The decoder waits for `end_input` after the compressed stream and rejects any further
    /// non-empty input.
    Reject,
}
