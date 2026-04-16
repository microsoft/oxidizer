// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

mod codec;
#[cfg(any(feature = "serialize", test))]
mod serialize;
mod tier;

pub use codec::{Codec, Encoder, infallible, infallible_owned};
#[cfg(any(feature = "serialize", test))]
pub(crate) use serialize::{PostcardCodec, PostcardEncoder};
pub(crate) use tier::TransformAdapter;
pub use tier::{TransformCodec, TransformEncoder};
