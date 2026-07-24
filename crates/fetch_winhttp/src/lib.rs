// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]

//! WinHTTP-based HTTP transport for the [`fetch`] HTTP client.
//!
//! This crate is a Windows-only custom transport that services `fetch`
//! [`HttpClient`] requests through the operating system's
//! [WinHTTP](https://learn.microsoft.com/en-us/windows/win32/winhttp/using-winhttp)
//! API, running in fully asynchronous mode.
//!
//! # Status
//!
//! This crate is a placeholder. Only the design exists so far; there is no
//! implementation yet. See [`docs/DESIGN.md`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch_winhttp/docs/DESIGN.md)
//! for the proposed architecture, threading/cancellation/error models, and the
//! test plan.
//!
//! [`fetch`]: https://docs.rs/fetch
//! [`HttpClient`]: https://docs.rs/fetch

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_winhttp/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_winhttp/favicon.ico")]

#[cfg(test)]
mod tests {
    /// The crate is a design-only placeholder with no implementation yet, so it
    /// exposes no behavior to exercise. This test gives the test runner a target
    /// to execute (`cargo nextest` treats an empty test set as an error).
    #[test]
    fn placeholder() {}
}
