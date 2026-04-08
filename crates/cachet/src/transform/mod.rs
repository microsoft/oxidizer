// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

mod codec;
#[cfg(any(feature = "serialize", test))]
mod serialize;
mod tier;

pub use codec::{Codec, Encoder, infallible};
#[cfg(any(feature = "serialize", test))]
pub(crate) use serialize::{BincodeCodec, BincodeEncoder};
pub(crate) use tier::TransformAdapter;
pub use tier::{TransformCodec, TransformEncoder};
