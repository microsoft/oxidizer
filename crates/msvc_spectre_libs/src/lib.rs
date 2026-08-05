// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Links the Spectre-mitigated C runtime libraries for Windows MSVC targets.
//!
//! Add this crate as a dependency to link its consumers with the
//! [Spectre-mitigated libraries] installed by Visual Studio. The crate has no
//! runtime API and does nothing for non-Windows or non-MSVC targets.
//!
//! Visual Studio Installer offers the libraries as optional components such as
//! `MSVC v143 - VS 2022 C++ x64/x86 Spectre-mitigated libs (Latest)`.
//!
//! # Missing libraries
//!
//! By default, the build script emits a warning and continues when it cannot
//! find the libraries. Enable the `error` feature to fail the build instead.
//!
//! [Spectre-mitigated libraries]: https://learn.microsoft.com/cpp/build/reference/qspectre

#[cfg(test)]
mod architecture;
