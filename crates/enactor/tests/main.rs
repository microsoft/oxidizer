// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for the public actor runtime API.

#![allow(clippy::renamed_function_params, reason = "test actors name the parameter after its message type")]

mod ask_timeout;
mod basic;
mod context;
mod exit;
mod lifecycle;
mod periodic;
mod pinning;
mod stream;
mod supervision;
