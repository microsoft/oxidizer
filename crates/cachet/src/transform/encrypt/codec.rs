// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Adapts a [`ValueProtector`] into a [`Codec`] pipeline stage.

use std::sync::Arc;

use bytesbuf::BytesView;

use super::{Rejection, Unprotected, ValueProtector};
use crate::Error;
use crate::telemetry::CacheTelemetry;
use crate::transform::{Codec, CodecContext, DecodeOutcome};

/// A byte-to-byte [`Codec`] stage that authenticates values with a [`ValueProtector`],
/// binding the storage key (carried by the [`CodecContext`]) as associated data.
///
/// Chained after serialization so it protects the serialized bytes; each backing tier is
/// authenticated independently on read. Being the stage that performs the authentication,
/// it *owns* the protection telemetry: an
/// [`AuthenticationFailed`](Rejection::AuthenticationFailed) rejection records
/// `cache.unprotect_failed`, while a [`Malformed`](Rejection::Malformed) one is a silent
/// miss. Both surface to the general codec pipeline as a plain
/// [`DecodeOutcome::SoftFailure`], so no crypto category leaks into the shared decode
/// vocabulary.
pub(crate) struct ProtectorCodec {
    protector: Arc<dyn ValueProtector>,
    telemetry: CacheTelemetry,
}

impl ProtectorCodec {
    pub(crate) fn new(protector: Arc<dyn ValueProtector>, telemetry: CacheTelemetry) -> Self {
        Self { protector, telemetry }
    }
}

impl Codec<BytesView, BytesView> for ProtectorCodec {
    fn encode(&self, ctx: &CodecContext<'_>, value: &BytesView) -> Result<BytesView, Error> {
        self.protector.protect(ctx.key(), value)
    }

    fn decode(&self, ctx: &CodecContext<'_>, value: BytesView) -> Result<DecodeOutcome<BytesView>, Error> {
        match self.protector.unprotect(ctx.key(), &value)? {
            Unprotected::Recovered(recovered) => Ok(DecodeOutcome::Value(recovered)),
            Unprotected::Rejected(Rejection::AuthenticationFailed) => {
                self.telemetry.record_unprotect_failure(std::any::type_name::<Self>());
                Ok(DecodeOutcome::SoftFailure)
            }
            Unprotected::Rejected(Rejection::Malformed) => Ok(DecodeOutcome::SoftFailure),
        }
    }
}
