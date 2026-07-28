// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! WinHTTP-based HTTP transport for [`fetch`].
//!
//! # Internal implementation detail
//!
//! This crate is an internal implementation detail of the SDK. It is not part
//! of the public API surface, must not be re-exported, and offers no stability
//! guarantees: anything may change in any release, including patch releases.
//!
//! # Status: skeleton
//!
//! The transport is **not implemented yet**. This crate currently contains only
//! the scaffolding and the design documentation under `docs/design/`. Those
//! documents define the intended API shape, how the transport plugs into the
//! [`fetch`] pipeline as a [`RequestHandler`], the strategy for bridging
//! `WinHTTP`'s asynchronous completion callbacks to Rust futures, and the
//! configuration mapping between [`fetch_options`]/[`fetch_tls`] and `WinHTTP`
//! (including the knobs that do not map cleanly).
//!
//! Scope, once implemented, is narrow: a transport that issues HTTP/1.1 or
//! HTTP/2 requests through the Windows [WinHTTP] stack, terminating TLS via
//! `SChannel` and the Windows certificate stores. It performs no higher-level
//! pipeline work (retry, hedging, metrics, logging) — that is layered on top by
//! [`fetch`].
//!
//! [`fetch`]: https://docs.rs/fetch
//! [`fetch_options`]: https://docs.rs/fetch_options
//! [`fetch_tls`]: https://docs.rs/fetch_tls
//! [`RequestHandler`]: https://docs.rs/http_extensions
//! [WinHTTP]: https://learn.microsoft.com/windows/win32/winhttp/winhttp-start-page

// Intentionally empty: see `docs/design/` for the planned architecture. The
// transport types, builder, async bridge, and configuration translation will be
// added here as implementation proceeds.
