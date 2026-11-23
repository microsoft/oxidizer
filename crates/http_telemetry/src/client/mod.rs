// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The client side, which produces telemetry about outgoing HTTP requests.

mod events;
mod layer;
mod request;
mod service;

pub use layer::Client;
