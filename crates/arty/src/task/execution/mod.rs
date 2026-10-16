// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Task preparation, execution wrappers, and result transport.
//!
//! Submission code owns admission decisions. These helpers construct executable
//! payloads and their result channels without deciding whether work is accepted.
//! Remote future construction is deferred to the destination worker. A deferred
//! task observed after shutdown stays pending until executor teardown disposes it
//! and publishes cancellation through its result channel.

mod preparation;

pub(crate) use preparation::{BoxedRemoteFutureFactory, DeferredTask, prepare_blocking, prepare_remote};
