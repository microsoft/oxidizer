// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(
    clippy::multiple_inherent_impl,
    reason = "Intentional - Windows extensions"
)]

// Windows-specific functionality. Some of these are stand-alone types, others are just
// impl blocks that add platform-specific public API surface to types defined elsewhere.

pub mod winsock;

mod as_native_primitive;
mod begin_result;
mod control_operation;
mod read_operation;
mod unbound_primitive;
mod write_operation;

pub use as_native_primitive::*;