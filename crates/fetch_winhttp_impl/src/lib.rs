// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Implementation of the [`fetch_winhttp`] WinHTTP transport.
//!
//! This crate carries the transport itself; [`fetch_winhttp`] is the package to
//! depend on, and re-exports everything here that is supported. Nothing in this
//! crate is a supported API: items are public only so that the facade can
//! re-export them, and they may be changed or removed at any time.
//!
//! The crate is empty on targets other than Windows.
//!
//! [`fetch_winhttp`]: https://docs.rs/fetch_winhttp

#![cfg_attr(docsrs, feature(doc_cfg))]
// The attribute this feature enables appears in the WinHTTP modules, which are
// configured out on other platforms, and in the placeholder module's tests. The
// feature is therefore declared only where a use of it survives configuration,
// because an unused feature declaration is itself an error.
#![cfg_attr(all(coverage_nightly, any(windows, test)), feature(coverage_attribute))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_winhttp/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_winhttp/favicon.ico")]

// Every module below implements the WinHTTP transport and is therefore
// configured out on other platforms. The gate is applied per item rather than
// at the crate root so that the crate still compiles to an instrumented, if
// empty, library elsewhere; see `linux` below.
#[cfg(windows)]
mod bindings;
#[cfg(windows)]
mod body;
#[cfg(windows)]
mod builder;
#[cfg(windows)]
mod callback;
#[cfg(windows)]
mod context;
#[cfg(windows)]
mod convert;
#[cfg(windows)]
mod error;
#[cfg(windows)]
mod error_labels;
#[cfg(windows)]
mod handle;
#[cfg(windows)]
#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod mocks;
#[cfg(windows)]
mod operation;
#[cfg(windows)]
mod options;
#[cfg(windows)]
mod query;
#[cfg(windows)]
mod request;
#[cfg(windows)]
mod response_headers;
#[cfg(windows)]
mod session;
#[cfg(windows)]
mod telemetry;
// Fixtures for the integration tests, benchmarks and examples that `fetch_winhttp`
// hosts. Scaffolding rather than transport code, so it is exempt from the coverage
// and mutation obligations the rest of the crate carries. Mutation exclusion is
// declared in `.cargo/mutants.toml` `exclude_globs` (the PR mutants CLI layers
// platform-specific excludes on top of that list).
#[cfg(all(windows, feature = "private-test-util"))]
#[cfg_attr(coverage_nightly, coverage(off))]
pub mod testing;
#[cfg(windows)]
mod tls;
#[cfg(windows)]
mod transport;

#[cfg(not(windows))]
mod linux;

#[cfg(windows)]
pub use builder::{HttpClientWinHttpExt, WinHttpDeps, WinHttpDepsBuilder};
#[cfg(windows)]
pub use tls::{WinHttpTlsConfig, WinHttpTlsConfigBuilder};
