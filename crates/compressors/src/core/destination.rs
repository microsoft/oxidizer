// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// What the caller does with the output it is handed.
///
/// A decompressor's exposure depends on this and on nothing else it can see: handing every chunk
/// straight on keeps nothing, so a stream of any length is safe, while accumulating means what the
/// engine produces is what the caller holds. Telling the engine which it is doing is what lets it
/// bound its own output against its own counter -- and stop producing at the bound, rather than
/// producing freely and leaving each caller to notice afterwards.
///
/// `pub` only to keep it off
/// [`CompressionInternal::pull`][super::CompressionInternal::pull]'s `private_interfaces` warning,
/// the same way [`Output`][super::Output] is: this module is private, so the name is unreachable
/// outside the crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    /// Each chunk is consumed and dropped, so only the chunk size is retained.
    Stream,

    /// The whole result is accumulated, so the engine's cumulative output is what is retained.
    Buffer,
}
