// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

mod read;
mod write;

pub(crate) use read::{WinHttpBodyReader, WinHttpResponseBody};
pub(crate) use write::{RequestBodyPlan, WinHttpBodyWriter, send_body};
