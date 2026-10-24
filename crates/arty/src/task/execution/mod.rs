// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Task preparation, execution wrappers, and result transport.
//!
//! Submission code owns admission decisions. These helpers construct executable
//! payloads and their result channels without deciding whether work is accepted.

mod preparation;

pub(crate) use preparation::{DeferredTask, RemoteFutureFactory, prepare_blocking, prepare_remote};
