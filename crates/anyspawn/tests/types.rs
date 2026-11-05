// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public type contracts, including builds without a runtime feature.

use std::fmt::Debug;
use std::marker::PhantomPinned;

use anyspawn::{JoinError, JoinHandle, Spawner, SpawnerBuilder};
use thread_aware::ThreadAware;

static_assertions::assert_impl_all!(Spawner: Clone, Debug, Send, Sync, ThreadAware);
static_assertions::assert_impl_all!(SpawnerBuilder: Clone, Debug, Send, Sync);
static_assertions::assert_impl_all!(JoinHandle<PhantomPinned>: Future<Output = Result<PhantomPinned, JoinError>>, Debug, Send, Unpin);
static_assertions::assert_not_impl_any!(JoinHandle<PhantomPinned>: Sync);
static_assertions::assert_impl_all!(JoinError: std::error::Error, Send, Sync);
