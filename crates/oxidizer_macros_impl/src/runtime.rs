// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

mod entrypoint_common;
mod entrypoint_main;
mod entrypoint_test;

pub use entrypoint_main::{impl_app_main, impl_oxidizer_app_main, impl_runtime_main};
pub use entrypoint_test::{impl_app_test, impl_oxidizer_app_test, impl_runtime_test};