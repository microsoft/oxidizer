// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Worked examples for [`rest_over_grpc`].
//!
//! The crate provides two service fixtures — both serving the *same*
//! `library.proto` service, differing only in how the handler is supplied (the
//! Handling layer) — and a set of runnable examples organized by the crate's
//! three architectural layers.
//!
//! # Fixtures
//!
//! - [`tonic_bridge`] — the common case. You implement your service only against
//!   `tonic`'s generated server trait, and `rest_over_grpc::build` emits a
//!   blanket `impl` that makes it a `rest_over_grpc` service too, so one
//!   implementation serves both gRPC and REST.
//! - [`custom`] — the hand-written path. You implement the generated service
//!   trait directly (no `tonic`), decoded from a `prost` descriptor, and can
//!   hand-write a bridge for another gRPC stack.
//!
//! Both expose a `Transcoder` for the identical REST surface, so they are an
//! apples-to-apples comparison; refer to them as [`tonic_bridge::Transcoder`] and
//! [`custom::Transcoder`].
//!
//! # Examples, by layer
//!
//! The runnable examples live under `examples/`, grouped by which architectural
//! layer they exercise:
//!
//! - `examples/serving/` — getting requests on and off the network
//!   (`tower_service`, `axum_app`, `streaming_response`, `custom_body_handling`).
//! - `examples/transcoding/` — calling the transcoder (`basic_transcode`,
//!   `custom_fallback`).
//! - `examples/handling/` — supplying the service logic (`direct_service`,
//!   `volo_bridge`, `client_streaming_upload`).

#![allow(clippy::allow_attributes, reason = "generated code carries #[allow] attributes to suppress lints")]

pub mod custom;
pub mod tonic_bridge;
