// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

mod accept;
mod accept_encoding;
mod accept_encoding_entry;
mod accept_entry;
mod accept_language;
mod accept_language_entry;
mod accept_scan;
mod allow;
mod content_coding;
mod host;
mod language_range;
mod media_range;
mod negotiation_members;
mod negotiation_parameter;
mod negotiation_token;
mod quality;
#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod recognition_test_support;
mod server;
mod shared;
mod vary;
mod vary_entry_view;
mod weighted_token_scan;

#[doc(inline)]
pub use accept::{Accept, AcceptOwned, AcceptView};
#[doc(inline)]
pub use accept_encoding::{AcceptEncoding, AcceptEncodingOwned, AcceptEncodingView};
#[doc(inline)]
pub use accept_encoding_entry::AcceptEncodingEntry;
#[doc(inline)]
pub use accept_entry::AcceptEntry;
#[doc(inline)]
pub use accept_language::{AcceptLanguage, AcceptLanguageOwned, AcceptLanguageView};
#[doc(inline)]
pub use accept_language_entry::AcceptLanguageEntry;
#[doc(inline)]
pub use allow::{Allow, AllowOwned, AllowView};
#[doc(inline)]
pub use content_coding::{ContentCoding, ContentCodingKind};
#[doc(inline)]
pub use host::{
    Host, HostKind, HostOwned, HostPortView, HostView, IpvFutureView, PortConversionError, PortConversionErrorKind, RegisteredNameView,
};
#[doc(inline)]
pub use language_range::LanguageRange;
#[doc(inline)]
pub use media_range::{MediaRange, MediaRangeKind};
#[doc(inline)]
pub use negotiation_parameter::{NegotiationParameter, NegotiationParameterValue, NegotiationParameters};
#[doc(inline)]
pub use negotiation_token::NegotiationToken;
#[doc(inline)]
pub use quality::{InexactQuality, InvalidQuality, Quality, QualityView};
#[doc(inline)]
pub use server::{Server, ServerOwned, ServerView};
#[doc(inline)]
pub use vary::{Vary, VaryOwned, VaryView};
#[doc(inline)]
pub use vary_entry_view::VaryEntryView;

use super::{FieldNameView, MethodView};
