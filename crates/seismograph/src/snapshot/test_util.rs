// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Snapshot fixtures with controlled events, sources, and symbol resolution.

use super::{DecodedSnapshot, Snapshot, Symbol};
use crate::Error;

/// Resolves the supplied event frames and encodes the resulting snapshot.
///
/// # Errors
///
/// Returns container validation or allocation errors.
///
/// # Panics
///
/// Propagates a panic from `resolve`.
pub fn capture(snapshot: DecodedSnapshot, resolve: impl FnMut(&mut Symbol)) -> Result<Snapshot, Error> {
    encode(&super::with_symbols(snapshot, resolve))
}

/// Encodes supplied events, sources, and symbols without consulting the host.
///
/// # Errors
///
/// Returns container validation or allocation errors.
pub fn encode(snapshot: &DecodedSnapshot) -> Result<Snapshot, Error> {
    super::encode_snapshot(snapshot)
}
