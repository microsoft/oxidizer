// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::pal::{CompletionNotification, ElementaryOperationKey};

#[derive(Debug)]
pub struct CompletionNotificationImpl;

impl CompletionNotification for CompletionNotificationImpl {
    #[cfg_attr(test, mutants::skip)] // Linux is just a placeholder to validate "builds, not runs".
    fn elementary_operation_key(&self) -> ElementaryOperationKey {
        todo!()
    }

    #[cfg_attr(test, mutants::skip)] // Linux is just a placeholder to validate "builds, not runs".
    fn result(&self) -> crate::Result<u32> {
        todo!()
    }

    #[cfg_attr(test, mutants::skip)] // Linux is just a placeholder to validate "builds, not runs".
    fn is_wake_up_signal(&self) -> bool {
        todo!()
    }
}