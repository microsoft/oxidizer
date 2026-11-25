// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The server side, which produces telemetry about incoming HTTP requests.

mod events;
mod excluded_paths;
mod layer;
mod request;
mod response_body;
mod route_fields;
mod service;

pub use layer::Server;
