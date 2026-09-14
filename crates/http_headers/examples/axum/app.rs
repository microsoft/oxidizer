// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use http_headers::headers::{CacheControl, UserAgent};
use http_headers::sink::FieldSinkExt;
use serde::Serialize;

#[derive(Serialize)]
struct Greeting {
    message: &'static str,
    user_agent: String,
}

type HandlerError = (StatusCode, String);

async fn greeting(headers: HeaderMap) -> Result<Response, HandlerError> {
    let user_agent = UserAgent::view(&headers)
        .map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))?
        .map(|value| value.as_str().map(str::to_owned))
        .transpose()
        .map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))?
        .unwrap_or_else(|| "unknown".to_owned());

    let mut response = Json(Greeting {
        message: "hello from http_headers",
        user_agent,
    })
    .into_response();

    response
        .headers_mut()
        .set_cache_control(CacheControl::private().max_age(Duration::from_mins(1)))
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;

    Ok(response)
}

pub(crate) fn router() -> Router {
    Router::new().route("/", get(greeting))
}
