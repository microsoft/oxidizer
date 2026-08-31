// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

mod read;
mod write;

pub(crate) use read::{WinHttpBodyReader, WinHttpResponseBody, declared_body_length};
pub(crate) use write::{RequestBodyFraming, WinHttpBodyWriter, send_body};
