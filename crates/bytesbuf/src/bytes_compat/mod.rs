// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compatibility with types from the `bytes` packages.

mod buf;
mod from_bytes;
mod to_bytes;
mod view;

pub(crate) use from_bytes::bytes_storage_to_spans_reversed;
