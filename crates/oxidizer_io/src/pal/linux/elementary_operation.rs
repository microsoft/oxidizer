// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;

use crate::pal::{ElementaryOperation, ElementaryOperationKey};

#[derive(Debug)]
pub struct ElementaryOperationImpl;

impl ElementaryOperation for ElementaryOperationImpl {
    #[cfg_attr(test, mutants::skip)] // Linux is just a placeholder to validate "builds, not runs".
    fn key(self: Pin<&Self>) -> ElementaryOperationKey {
        todo!()
    }
}