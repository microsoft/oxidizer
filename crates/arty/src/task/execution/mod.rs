// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Task preparation, execution wrappers, and result transport.
//!
//! Submission code owns admission decisions. These helpers construct executable
//! payloads and their result channels without deciding whether work is accepted.

mod local;
mod preparation;
mod remote;
mod result;
mod storage;

pub(super) use preparation::prepare_local;
pub(crate) use preparation::{BoxedRemoteFutureFactory, prepare_blocking, prepare_remote, prepare_remote_on_worker};
pub(super) use result::TaskResult;
