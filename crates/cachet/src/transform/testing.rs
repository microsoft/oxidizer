// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Test utilities for codec testing.

use std::fmt::Debug;
use std::marker::PhantomData;

use super::codec::DecodeOutcome;
use crate::{Codec, CodecContext, Error};

/// A mock codec for testing that uses identity encoding and a configurable decode outcome.
///
/// Encode is identity (clones the value). Decode returns the configured [`DecodeOutcome`].
///
/// # Examples
///
/// ```ignore
/// use cachet::transform::testing::MockCodec;
///
/// // A codec that always succeeds
/// let codec = MockCodec::<i32>::value();
///
/// // A codec that always returns a soft failure
/// let codec = MockCodec::<i32>::soft_failure();
/// ```
pub(crate) struct MockCodec<T> {
    soft_failure: bool,
    _phantom: PhantomData<T>,
}

impl<T> MockCodec<T> {
    /// Creates a mock codec that decodes successfully (returns the value as-is).
    #[must_use]
    pub(crate) fn value() -> Self {
        Self {
            soft_failure: false,
            _phantom: PhantomData,
        }
    }

    /// Creates a mock codec that always returns [`DecodeOutcome::SoftFailure`].
    #[must_use]
    pub(crate) fn soft_failure() -> Self {
        Self {
            soft_failure: true,
            _phantom: PhantomData,
        }
    }
}

impl<T: Clone + Send + Sync> Codec<T, T> for MockCodec<T> {
    fn encode(&self, _ctx: &CodecContext<'_>, value: &T) -> Result<T, Error> {
        Ok(value.clone())
    }

    fn decode(&self, _ctx: &CodecContext<'_>, value: T) -> Result<DecodeOutcome<T>, Error> {
        if self.soft_failure {
            Ok(DecodeOutcome::SoftFailure)
        } else {
            Ok(DecodeOutcome::Value(value))
        }
    }
}

impl<T> Debug for MockCodec<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockCodec").field("soft_failure", &self.soft_failure).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_codec_roundtrips() {
        let codec = MockCodec::<i32>::value();
        assert_eq!(codec.encode(&CodecContext::keyless(), &42).unwrap(), 42);
        assert!(matches!(
            codec.decode(&CodecContext::keyless(), 42).unwrap(),
            DecodeOutcome::Value(42)
        ));
    }

    #[test]
    fn soft_failure_codec_decodes_to_soft_failure() {
        let codec = MockCodec::<i32>::soft_failure();
        assert!(matches!(
            codec.decode(&CodecContext::keyless(), 42).unwrap(),
            DecodeOutcome::SoftFailure
        ));
    }

    #[test]
    fn soft_failure_codec_encodes_normally() {
        let codec = MockCodec::<i32>::soft_failure();
        assert_eq!(codec.encode(&CodecContext::keyless(), &42).unwrap(), 42);
    }

    #[test]
    fn debug_output() {
        let codec = MockCodec::<i32>::value();
        let debug = format!("{codec:?}");
        assert!(debug.contains("MockCodec"));

        let codec = MockCodec::<i32>::soft_failure();
        let debug = format!("{codec:?}");
        assert!(debug.contains("soft_failure: true"));
    }
}
