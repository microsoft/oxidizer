// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(miri, expect(dead_code, reason = "too much noise to satisfy Miri's expectations"))]

use std::pin::Pin;
use std::task::{Context, Poll};

use crate::{Recovery, RecoveryInfo};

/// A tower service whose `poll_ready` always returns an error.
///
/// Useful for testing that resilience middleware correctly propagates
/// inner service readiness failures.
#[derive(Clone, Debug)]
pub(crate) struct FailReadyService;

impl tower_service::Service<String> for FailReadyService {
    type Response = String;
    type Error = String;
    type Future = Pin<Box<dyn Future<Output = Result<String, String>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Err("inner service unavailable".to_string()))
    }

    fn call(&mut self, _req: String) -> Self::Future {
        unreachable!("call should not be invoked when poll_ready fails")
    }
}

#[derive(Debug)]
pub(crate) struct RecoverableType(RecoveryInfo);

impl Recovery for RecoverableType {
    fn recovery(&self) -> RecoveryInfo {
        self.0.clone()
    }
}

impl From<RecoveryInfo> for RecoverableType {
    fn from(recovery: RecoveryInfo) -> Self {
        Self(recovery)
    }
}
