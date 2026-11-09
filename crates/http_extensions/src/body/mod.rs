// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! HTTP body types for requests and responses.
//!
//! [`HttpBody`] supports:
//!
//! - Text and string data.
//! - Binary data.
//! - JSON data with the `json` feature.
//! - Streaming content.
//! - Empty bodies.
//! - Custom body implementations.
//!
//! Create bodies with [`HttpBodyBuilder`].
//! All bodies implement the standard `http_body::Body` trait.

mod body;
mod builder;
pub(crate) mod options;
mod timeout_body;
pub use body::{Content, HttpBody, StreamingBody};
pub use builder::HttpBodyBuilder;
pub use options::HttpBodyOptions;
