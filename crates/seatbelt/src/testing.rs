// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(miri, expect(dead_code, reason = "too much noise to satisfy Miri's expectations"))]

use crate::{Recovery, RecoveryInfo};

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
