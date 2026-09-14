// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Conditional request and HTTP-date header types.

mod if_match;
mod if_modified_since;
mod if_none_match;
mod if_range;
mod if_unmodified_since;
mod last_modified;
mod shared;

#[doc(inline)]
pub use if_match::{IfMatch, IfMatchOwned, IfMatchView};
#[doc(inline)]
pub use if_modified_since::{IfModifiedSince, IfModifiedSinceOwned, IfModifiedSinceView};
#[doc(inline)]
pub use if_none_match::{IfNoneMatch, IfNoneMatchOwned, IfNoneMatchView};
#[doc(inline)]
pub use if_range::{IfRange, IfRangeOwned, IfRangeValueView, IfRangeView};
#[doc(inline)]
pub use if_unmodified_since::{IfUnmodifiedSince, IfUnmodifiedSinceOwned, IfUnmodifiedSinceView};
#[doc(inline)]
pub use last_modified::{LastModified, LastModifiedOwned, LastModifiedView};
#[doc(inline)]
pub use shared::ConditionalTagView;
