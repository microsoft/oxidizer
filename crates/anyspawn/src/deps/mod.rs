// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime dependencies for constructing a [`Spawner`](crate::Spawner).

#[cfg(any(test, feature = "arty"))]
mod arty;
#[cfg(any(test, feature = "tokio"))]
mod tokio;

#[cfg(any(test, feature = "arty"))]
pub use arty::ArtyDeps;
#[cfg(any(test, feature = "tokio"))]
pub use tokio::TokioDeps;
