// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

mod completion_notification;
mod completion_queue;
mod completion_queue_waker;
mod elementary_operation;
mod platform;
mod primitive;

pub use completion_notification::*;
pub use completion_queue::*;
pub use completion_queue_waker::*;
pub use elementary_operation::*;
pub use platform::*;
pub use primitive::*;

pub const fn static_build_target_platform() -> BuildTargetPlatform {
    BuildTargetPlatform {}
}