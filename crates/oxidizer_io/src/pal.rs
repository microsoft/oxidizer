// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

mod abstractions;
mod default_memory_pool;
mod facade;
pub use abstractions::*;
pub use default_memory_pool::*;
pub use facade::*;

#[cfg(test)]
mod mocks;
#[cfg(test)]
pub use mocks::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

/// Singleton instance of `BuildTargetPlatform`, used by public API types
/// to hook up to the correct PAL implementation.
///
/// Internal types in this crate may also use other (e.g. mock) platforms,
/// with the instance typically received via ctor parameter.
pub static BUILD_TARGET_PLATFORM: BuildTargetPlatform = static_build_target_platform();