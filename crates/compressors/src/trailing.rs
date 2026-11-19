// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// How a single-stream decoder handles bytes after the compressed stream.
///
/// In multi-stream mode, subsequent bytes are always interpreted as another compressed stream and
/// must be valid. This policy applies when multi-stream decoding is disabled.
///
/// [`Reject`][Self::Reject] is the default. Silently discarding trailing bytes is a parser
/// differential. A proxy or scanner can inspect only the prefix while a downstream component reads
/// the suffix. Use [`Ignore`][Self::Ignore] for a container that places other data after the
/// compressed stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum TrailingData {
    /// Stop after the compressed stream and ignore whatever follows it.
    ///
    /// The decoder reports that it is done at the end of the stream and never looks at the bytes
    /// after it.
    Ignore,

    /// Require the compressed stream to end exactly at end of input.
    ///
    /// The decoder waits for `end_input` after the compressed stream and rejects any further
    /// non-empty input.
    #[default]
    Reject,
}
