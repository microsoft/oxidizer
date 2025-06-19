// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use derive_more::Display;

use crate::pal::Primitive;

#[derive(Clone, Debug, Display)]
#[display("placeholder")]
pub struct PrimitiveImpl;

impl Primitive for PrimitiveImpl {
    #[cfg_attr(test, mutants::skip)] // Linux is just a placeholder to validate "builds, not runs".
    fn close(&self) {
        todo!()
    }
}