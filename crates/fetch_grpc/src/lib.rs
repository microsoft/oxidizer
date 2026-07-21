// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_grpc/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_grpc/favicon.ico")]

//! A gRPC transport backed by the `fetch` HTTP client.
//!
//! This crate adapts a `fetch` HTTP client into a transport for the
//! [`grpc`](https://docs.rs/grpc) crate, so gRPC calls run over `fetch` and
//! benefit from its resilience and observability.
//!
//! It is currently an empty placeholder. A real implementation will follow.
