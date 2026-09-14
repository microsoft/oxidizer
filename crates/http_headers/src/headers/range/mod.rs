// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Byte range request, response, and capability headers.

mod accept_ranges;
mod content_range;
#[expect(
    clippy::module_inception,
    reason = "the file is named after the `Range` header it defines, matching this family's one-file-per-header-concept convention"
)]
mod range;
mod shared;

#[doc(inline)]
pub use accept_ranges::{AcceptRanges, AcceptRangesOwned, AcceptRangesView};
#[doc(inline)]
pub use content_range::{ByteContentRange, CompleteLength, ContentRange, ContentRangeOwned, ContentRangeView};
#[doc(inline)]
pub use range::{ByteRangeSpec, Range, RangeOwned, RangeView};
