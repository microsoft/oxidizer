// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Snmalloc state, separate from the shared snapshot header.

/// Snmalloc owner inventory and global range caches.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct State {
    /// Persistent endpoints, including unknown and previously published state.
    pub owners: Vec<crate::native::Owner>,
    /// Independently collected global range state.
    pub global: crate::native::GlobalState,
}
