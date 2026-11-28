// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! HTTP readiness adapters over a [`health::Monitor`].
//!
//! Each adapter reads the monitor for every request. `Healthy` responds with status `200 OK` and
//! the JSON body `true`. Any other state responds with `503 Service Unavailable` and the JSON body
//! `false`. Responses carry no component details and disable caching. The request body is ignored.
//! Applications own route registration, HTTP method selection, authentication, and authorization.

use ::http::header::{CACHE_CONTROL, CONTENT_TYPE, HeaderValue};
use ::http::{Response, StatusCode};
use health::Health;

fn readiness<B>(state: Health, body: impl FnOnce(&'static str) -> B) -> Response<B> {
    let (status, text) = match state {
        Health::Healthy => (StatusCode::OK, "true"),
        Health::Degraded | Health::Unhealthy => (StatusCode::SERVICE_UNAVAILABLE, "false"),
    };
    let mut response = Response::new(body(text));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(any(test, feature = "http-readiness-request"))]
pub mod readiness_request;

#[cfg(any(test, feature = "http-readiness-tower"))]
pub mod readiness_tower;

#[cfg(test)]
mod tests;
