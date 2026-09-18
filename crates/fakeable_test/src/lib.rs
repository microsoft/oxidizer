// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Testing utilities and helpers for the `fakeable` framework.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]
#![allow(
    clippy::missing_errors_doc,
    clippy::needless_pass_by_ref_mut,
    clippy::unused_self,
    missing_debug_implementations,
    missing_docs,
    unreachable_pub,
    reason = "Private end-to-end fixture exercises generated public APIs"
)]
#![expect(clippy::allow_attributes, reason = "for testing purposes")]

pub mod my_service;

pub mod generic_service;

pub mod my_service_mockall;

pub mod my_service_unused_fake;
