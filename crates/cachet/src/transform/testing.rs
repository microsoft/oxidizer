// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Test utilities for codec testing.

use std::fmt::Debug;
use std::marker::PhantomData;

use crate::{Codec, Encoder, Error};

use super::codec::DecodeOutcome;

/// A mock codec for testing that uses identity encoding and a configurable decode outcome.
///
/// Encode is identity (clones the value). Decode returns the configured [`DecodeOutcome`].
///
/// # Examples
///
/// ```
/// use cachet::{MockCodec, DecodeOutcome};
///
/// // A codec that always succeeds
/// let codec = MockCodec::<i32>::value();
///
/// // A codec that always returns a soft failure
/// let codec = MockCodec::<i32>::soft_failure("version mismatch");
/// ```
pub struct MockCodec<T> {
    soft_failure: Option<&'static str>,
    _phantom: PhantomData<T>,
}

impl<T> MockCodec<T> {
    /// Creates a mock codec that decodes successfully (returns the value as-is).
    #[must_use]
    pub fn value() -> Self {
        Self {
            soft_failure: None,
            _phantom: PhantomData,
        }
    }

    /// Creates a mock codec that always returns [`DecodeOutcome::SoftFailure`] with the given reason.
    #[must_use]
    pub fn soft_failure(reason: &'static str) -> Self {
        Self {
            soft_failure: Some(reason),
            _phantom: PhantomData,
        }
    }
}

impl<T: Clone + Send + Sync> Encoder<T, T> for MockCodec<T> {
    fn encode(&self, value: &T) -> Result<T, Error> {
        Ok(value.clone())
    }
}

impl<T: Clone + Send + Sync> Codec<T, T> for MockCodec<T> {
    fn decode(&self, value: T) -> Result<DecodeOutcome<T>, Error> {
        match self.soft_failure {
            Some(reason) => Ok(DecodeOutcome::SoftFailure(reason)),
            None => Ok(DecodeOutcome::Value(value)),
        }
    }
}

impl<T> Debug for MockCodec<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockCodec")
            .field("soft_failure", &self.soft_failure)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_codec_roundtrips() {
        let codec = MockCodec::<i32>::value();
        assert_eq!(codec.encode(&42).unwrap(), 42);
        let DecodeOutcome::Value(v) = codec.decode(42).unwrap() else {
            panic!("expected Value");
        };
        assert_eq!(v, 42);
    }

    #[test]
    fn soft_failure_codec_decodes_to_soft_failure() {
        let codec = MockCodec::<i32>::soft_failure("bad data");
        assert!(matches!(
            codec.decode(42).unwrap(),
            DecodeOutcome::SoftFailure("bad data")
        ));
    }

    #[test]
    fn soft_failure_codec_encodes_normally() {
        let codec = MockCodec::<i32>::soft_failure("bad data");
        assert_eq!(codec.encode(&42).unwrap(), 42);
    }

    #[test]
    fn debug_output() {
        let codec = MockCodec::<i32>::value();
        let debug = format!("{codec:?}");
        assert!(debug.contains("MockCodec"));

        let codec = MockCodec::<i32>::soft_failure("reason");
        let debug = format!("{codec:?}");
        assert!(debug.contains("reason"));
    }
}
